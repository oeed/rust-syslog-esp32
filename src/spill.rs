//! On-flash, reboot-persistent spill buffer for log records that could not be delivered.
//!
//! Records are length-framed with a CRC32 so a write torn by power loss is detected and
//! discarded on the next read. The store is bounded by both a record count and a total byte
//! size ([`MAX_SPILL_BYTES`]) so it can always be loaded into RAM to drain it without
//! exhausting the (small) heap — the partition may be much larger than this cap. It is
//! written to a filesystem path the caller mounts (a FAT/wear-levelled partition on the
//! ESP32); durability comes from that filesystem plus the atomic temp-file rename used by
//! [`SpillStore::replace_with`].

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

const MAGIC: u8 = 0xA5;
const VERSION: u8 = 1;
const FLAG_WALL_PRESENT: u8 = 0b0000_0001;

/// Header bytes preceding the message: magic, version, flags, severity, uptime(8),
/// boot_epoch(8), wall(8), msg_len(2).
const HEADER_LEN: usize = 1 + 1 + 1 + 1 + 8 + 8 + 8 + 2;
const CRC_LEN: usize = 4;

/// Upper bound on a single message, mirroring the original UDP transport's 1 KiB cap.
const MAX_MSG_LEN: usize = 1024;

/// Hard cap on the total spilled bytes kept on flash. The whole buffer is read into RAM to
/// drain it, so this MUST stay well under the device's free heap — it bounds memory, not the
/// partition (which can be larger). At ~24 KiB this holds a few hundred typical log lines.
const MAX_SPILL_BYTES: usize = 24 * 1024;

/// A single buffered log event.
pub struct Record {
    /// Syslog numeric severity (0..=7).
    pub severity: u8,
    /// Monotonic uptime in microseconds at capture.
    pub uptime_us: u64,
    /// Boot epoch at capture (see [`crate::clock`]).
    pub boot_epoch: u64,
    /// Absolute capture time in Unix microseconds, or `0` if it was unknown when stored.
    pub wall_us: u64,
    /// The raw log message bytes.
    pub msg: Vec<u8>,
}

impl Record {
    /// Number of bytes this record occupies on disk once framed.
    fn encoded_len(&self) -> usize {
        HEADER_LEN + self.msg.len().min(MAX_MSG_LEN) + CRC_LEN
    }

    fn to_bytes(&self) -> Vec<u8> {
        let msg_len = self.msg.len().min(MAX_MSG_LEN);
        let msg = &self.msg[..msg_len];

        let mut buf = Vec::with_capacity(HEADER_LEN + msg_len + CRC_LEN);
        buf.push(MAGIC);
        buf.push(VERSION);
        buf.push(if self.wall_us != 0 { FLAG_WALL_PRESENT } else { 0 });
        buf.push(self.severity);
        buf.extend_from_slice(&self.uptime_us.to_le_bytes());
        buf.extend_from_slice(&self.boot_epoch.to_le_bytes());
        buf.extend_from_slice(&self.wall_us.to_le_bytes());
        buf.extend_from_slice(&(msg_len as u16).to_le_bytes());
        buf.extend_from_slice(msg);

        let crc = crc32(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Parse one record from the front of `data`, returning it and the number of bytes
    /// consumed. Returns `None` on the first malformed framing (e.g. a torn tail write),
    /// which the reader treats as the end of valid data.
    fn parse(data: &[u8]) -> Option<(Record, usize)> {
        if data.len() < HEADER_LEN + CRC_LEN {
            return None;
        }
        if data[0] != MAGIC || data[1] != VERSION {
            return None;
        }
        let severity = data[3];
        let uptime_us = u64::from_le_bytes(data[4..12].try_into().ok()?);
        let boot_epoch = u64::from_le_bytes(data[12..20].try_into().ok()?);
        let wall_us = u64::from_le_bytes(data[20..28].try_into().ok()?);
        let msg_len = u16::from_le_bytes(data[28..30].try_into().ok()?) as usize;
        if msg_len > MAX_MSG_LEN {
            return None;
        }
        let total = HEADER_LEN + msg_len + CRC_LEN;
        if data.len() < total {
            return None;
        }
        let stored_crc = u32::from_le_bytes(data[HEADER_LEN + msg_len..total].try_into().ok()?);
        if crc32(&data[..HEADER_LEN + msg_len]) != stored_crc {
            return None;
        }
        let msg = data[HEADER_LEN..HEADER_LEN + msg_len].to_vec();
        Some((
            Record {
                severity,
                uptime_us,
                boot_epoch,
                wall_us,
                msg,
            },
            total,
        ))
    }
}

fn total_bytes(records: &[Record]) -> usize {
    records.iter().map(Record::encoded_len).sum()
}

pub struct SpillStore {
    path: PathBuf,
    tmp_path: PathBuf,
    max_records: usize,
    count: usize,
    bytes: usize,
}

impl SpillStore {
    /// Open the spill file at `path`, recovering any records left by a previous boot.
    ///
    /// If the existing file is unreadable, or larger than [`MAX_SPILL_BYTES`] (which can
    /// happen after a crash, a power-loss, or an older build with a different cap), it is
    /// discarded rather than loaded — loading it could exhaust the heap and crash on boot.
    pub fn open(path: &str, max_records: usize) -> SpillStore {
        let path = PathBuf::from(path);
        let tmp_path = path.with_extension("tmp");
        // A temp file left behind by a crash mid-rewrite is stale; discard it.
        let _ = std::fs::remove_file(&tmp_path);

        let mut store = SpillStore {
            path,
            tmp_path,
            max_records,
            count: 0,
            bytes: 0,
        };

        let records = read_records(&store.path);
        if records.is_empty() {
            // Empty, unreadable, or over-cap (read_records refuses oversized files): start
            // from a clean slate so a corrupt/oversized buffer can't wedge the device.
            store.clear();
        } else {
            // Rewrite exactly the records we could parse (dropping any torn tail) and set
            // the counters from them.
            store.replace_with(&records);
        }
        store
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Append a record, evicting the oldest first so the store stays within both the record
    /// and byte caps.
    pub fn append(&mut self, record: &Record) {
        let len = record.encoded_len();
        let over_count = self.max_records > 0 && self.count >= self.max_records;
        let over_bytes = self.bytes + len > MAX_SPILL_BYTES;
        if over_count || over_bytes {
            self.trim_to_fit(len);
        }
        match OpenOptions::new().create(true).append(true).open(&self.path) {
            Ok(mut file) => {
                if file.write_all(&record.to_bytes()).is_ok() {
                    let _ = file.flush();
                    self.count += 1;
                    self.bytes += len;
                }
            }
            Err(_) => {
                // Filesystem unavailable; nothing else we can do but drop.
            }
        }
    }

    pub fn read_all(&self) -> Vec<Record> {
        read_records(&self.path)
    }

    /// Atomically replace the file contents with `records` (temp file + rename).
    pub fn replace_with(&mut self, records: &[Record]) {
        if let Ok(mut file) = File::create(&self.tmp_path) {
            let mut ok = true;
            for record in records {
                if file.write_all(&record.to_bytes()).is_err() {
                    ok = false;
                    break;
                }
            }
            let _ = file.flush();
            drop(file);
            if ok && std::fs::rename(&self.tmp_path, &self.path).is_ok() {
                self.count = records.len();
                self.bytes = total_bytes(records);
                return;
            }
        }
        let _ = std::fs::remove_file(&self.tmp_path);
    }

    pub fn clear(&mut self) {
        if std::fs::remove_file(&self.path).is_ok() || !self.path.exists() {
            self.count = 0;
            self.bytes = 0;
            return;
        }
        // Removal failed but the file is still present: truncate it to empty so its
        // already-delivered records can't be re-drained and duplicated on the server.
        if File::create(&self.path).is_ok() {
            self.count = 0;
            self.bytes = 0;
        }
    }

    /// Drop the oldest records until `incoming` more bytes fit under both caps.
    fn trim_to_fit(&mut self, incoming: usize) {
        let mut records = self.read_all();
        let mut total = total_bytes(&records);
        let byte_limit = MAX_SPILL_BYTES.saturating_sub(incoming);
        let count_limit = if self.max_records > 0 {
            self.max_records.saturating_sub(1)
        } else {
            usize::MAX
        };

        let mut drop_n = 0;
        while drop_n < records.len() && (records.len() - drop_n > count_limit || total > byte_limit) {
            total -= records[drop_n].encoded_len();
            drop_n += 1;
        }
        records.drain(0..drop_n);
        self.replace_with(&records);
    }
}

fn read_records(path: &PathBuf) -> Vec<Record> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };

    // Read with a fixed buffer into a single, bounded allocation — never `read_to_end`, which
    // reserves the file's reported size up front and aborts the process on a small heap if the
    // file (or its metadata) is large. `try_reserve` degrades to "nothing recovered" instead.
    let mut data: Vec<u8> = Vec::new();
    if data.try_reserve_exact(MAX_SPILL_BYTES).is_err() {
        return Vec::new();
    }
    let mut chunk = [0u8; 512];
    loop {
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if data.len() + n > MAX_SPILL_BYTES {
                    // Oversized: refuse to load it. `open` treats an empty result for a
                    // non-empty file as a signal to reset, avoiding an out-of-memory abort.
                    return Vec::new();
                }
                data.extend_from_slice(&chunk[..n]);
            }
            Err(_) => return Vec::new(),
        }
    }

    let mut records = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        match Record::parse(&data[offset..]) {
            Some((record, used)) => {
                records.push(record);
                offset += used;
            }
            // Malformed framing: stop here (treat the remainder as a torn tail).
            None => break,
        }
    }
    records
}

/// CRC-32 (IEEE 802.3, reflected). Bitwise — small records, so a table is unwarranted.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod test {
    use super::*;

    fn record(msg: &str, wall_us: u64) -> Record {
        Record {
            severity: 6,
            uptime_us: 123_456,
            boot_epoch: 0xDEAD_BEEF,
            wall_us,
            msg: msg.as_bytes().to_vec(),
        }
    }

    #[test]
    fn round_trips_a_record() {
        let bytes = record("hello", 1_700_000_000_000_000).to_bytes();
        let (parsed, used) = Record::parse(&bytes).expect("parse");
        assert_eq!(used, bytes.len());
        assert_eq!(used, record("hello", 0).encoded_len());
        assert_eq!(parsed.msg, b"hello");
        assert_eq!(parsed.wall_us, 1_700_000_000_000_000);
        assert_eq!(parsed.boot_epoch, 0xDEAD_BEEF);
        assert_eq!(parsed.severity, 6);
    }

    #[test]
    fn rejects_corrupt_crc() {
        let mut bytes = record("hello", 0).to_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        assert!(Record::parse(&bytes).is_none());
    }

    #[test]
    fn stops_at_torn_tail() {
        let mut data = record("one", 0).to_bytes();
        let good = data.len();
        // A second record truncated mid-write.
        let partial = record("two", 0).to_bytes();
        data.extend_from_slice(&partial[..partial.len() / 2]);

        let mut offset = 0;
        let mut count = 0;
        while let Some((_, used)) = Record::parse(&data[offset..]) {
            count += 1;
            offset += used;
        }
        assert_eq!(count, 1);
        assert_eq!(offset, good);
    }
}
