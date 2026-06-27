//! Syslog for ESP32 (TCP, RFC 5424) with a reboot-persistent spill buffer.
//!
//! Messages are formatted in RFC 5424 and delivered to a syslog server over TCP using
//! RFC 6587 octet-counted framing. A dedicated worker thread owns the connection; when the
//! server is unreachable, records are spilled to a filesystem path (a flash partition the
//! caller mounts) and drained oldest-first once the connection is re-established, so logs
//! produced during a connectivity outage — or before the network came up at boot — survive
//! across reboots and are recovered later.
//!
//! Timestamps reflect when each event actually occurred, not when it was delivered. See
//! [`clock`] for how absolute time is reconstructed on a device with no battery-backed RTC;
//! call [`sync_wall_clock`] whenever a trusted wall-clock becomes available (e.g. SNTP).
//!
//! Use with the `log` crate via [`init_tcp_ipv4`].
//!
//! ```no_run
//! use log::LevelFilter;
//! use syslog_esp32::{Facility, init_tcp_ipv4};
//!
//! # fn main() -> Result<(), syslog_esp32::Error> {
//! init_tcp_ipv4(
//!     Some("esp32"),
//!     "myprogram",
//!     Facility::LOG_USER,
//!     LevelFilter::Info,
//!     "192.168.1.10:514".parse().unwrap(),
//!     "/spill/log.bin",
//!     500,
//! )?;
//!
//! log::info!("hello world");
//! # Ok(())
//! # }
//! ```

extern crate log;
extern crate time;

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use esp_idf_svc::log::EspLogger;
use log::{Level, Log, Metadata, Record};
use time::OffsetDateTime;

pub mod clock;
mod errors;
mod facility;
mod format;
mod spill;

pub use clock::sync_wall_clock;
pub use errors::*;
pub use facility::Facility;
pub use format::Severity;
pub use format::{Formatter5424, LogFormat};

use spill::SpillStore;

pub type Priority = u8;

/// How many buffered records `log()` can hand to the worker before back-pressure drops
/// new ones. The worker offloads to flash, so this only needs to absorb bursts while the
/// worker is briefly busy (e.g. blocked in a connect attempt).
const CHANNEL_CAPACITY: usize = 256;

/// Reconnect cadence and per-operation timeouts for the worker thread.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// A captured log event handed from `log()` to the worker thread. The absolute time is
/// resolved later (at spill or send), so only the monotonic uptime is captured here.
struct Outgoing {
    severity: Severity,
    uptime_us: u64,
    msg: String,
}

impl Outgoing {
    /// Freeze this event into a spill record, stamping the absolute time if it is already
    /// known. If not, the uptime and boot epoch are retained so it can still be resolved
    /// should the clock sync later in this same boot.
    fn to_record(&self) -> spill::Record {
        spill::Record {
            severity: self.severity as u8,
            uptime_us: self.uptime_us,
            boot_epoch: clock::boot_epoch(),
            wall_us: clock::resolve_wall_micros(self.uptime_us).unwrap_or(0),
            msg: self.msg.as_bytes().to_vec(),
        }
    }
}

/// Global logger: mirrors to the ESP serial logger and forwards to the TCP/spill worker.
pub struct SyslogLogger {
    sender: SyncSender<Outgoing>,
    esp_logger: Arc<Mutex<EspLogger>>,
}

impl Log for SyslogLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level() && metadata.level() <= log::STATIC_MAX_LEVEL
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        if let Ok(esp_logger) = self.esp_logger.try_lock() {
            esp_logger.log(record);
        }

        let mut msg = format!("{}", record.args());
        truncate_to_char_boundary(&mut msg, 1024);

        let out = Outgoing {
            severity: severity_from_level(record.level()),
            uptime_us: clock::uptime_micros(),
            msg,
        };
        // Drop on a full queue; the worker spills to flash, so a full queue only happens
        // under a momentary burst, never during a sustained outage.
        let _ = self.sender.try_send(out);
    }

    fn flush(&self) {}
}

fn severity_from_level(level: Level) -> Severity {
    match level {
        Level::Error => Severity::LOG_ERR,
        Level::Warn => Severity::LOG_WARNING,
        Level::Info => Severity::LOG_INFO,
        Level::Debug => Severity::LOG_DEBUG,
        Level::Trace => Severity::LOG_DEBUG,
    }
}

fn truncate_to_char_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

/// Render a stored record as a single RFC 5424 line, resolving its timestamp:
/// a frozen absolute time if present; otherwise the SNTP anchor, but only for a record
/// captured in the current boot; otherwise the NILVALUE.
fn format_record(formatter: &Formatter5424, record: &spill::Record) -> Vec<u8> {
    let wall_us = if record.wall_us != 0 {
        Some(record.wall_us)
    } else if record.boot_epoch == clock::boot_epoch() {
        clock::resolve_wall_micros(record.uptime_us)
    } else {
        None
    };
    let timestamp = wall_us.and_then(|us| OffsetDateTime::from_unix_timestamp_nanos(us as i128 * 1000).ok());

    let msg = String::from_utf8_lossy(&record.msg);
    let severity = Severity::from_u8(record.severity);

    let mut buf = Vec::with_capacity(64 + record.msg.len());
    let data: format::StructuredData = BTreeMap::new();
    let _ = formatter.format(&mut buf, severity, (1u32, data, msg.as_ref()), timestamp);
    buf
}

/// Send one syslog message using RFC 6587 octet counting (`MSG-LEN SP MESSAGE`).
fn send_frame(stream: &mut TcpStream, line: &[u8]) -> io::Result<()> {
    let header = format!("{} ", line.len());
    stream.write_all(header.as_bytes())?;
    stream.write_all(line)?;
    Ok(())
}

fn connect(addr: &SocketAddr) -> Option<TcpStream> {
    match TcpStream::connect_timeout(addr, CONNECT_TIMEOUT) {
        Ok(stream) => {
            let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
            let _ = stream.set_nodelay(true);
            Some(stream)
        }
        Err(_) => None,
    }
}

/// Send all spilled records oldest-first over `stream`. On success the store is cleared;
/// on a send failure partway through, the un-sent remainder is kept for the next attempt.
fn drain(spill: &mut SpillStore, stream: &mut TcpStream, formatter: &Formatter5424) -> io::Result<()> {
    let records = spill.read_all();
    if records.is_empty() {
        return Ok(());
    }

    let mut sent = 0;
    for record in &records {
        let line = format_record(formatter, record);
        if send_frame(stream, &line).is_err() {
            break;
        }
        sent += 1;
    }

    if sent == records.len() {
        spill.clear();
        Ok(())
    } else {
        // Only rewrite when progress was made; with sent == 0 the file already holds
        // exactly these records, so rewriting it would churn flash for nothing.
        if sent > 0 {
            spill.replace_with(&records[sent..]);
        }
        Err(io::Error::new(io::ErrorKind::Other, "partial drain"))
    }
}

fn run_worker(rx: Receiver<Outgoing>, addr: SocketAddr, formatter: Formatter5424, mut spill: SpillStore) {
    let mut stream: Option<TcpStream> = None;

    loop {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(out) => {
                let record = out.to_record();
                match stream.as_mut() {
                    Some(active) => {
                        let line = format_record(&formatter, &record);
                        if send_frame(active, &line).is_err() {
                            // Connection went away mid-send: drop it and spill this record.
                            stream = None;
                            spill.append(&record);
                        }
                    }
                    None => spill.append(&record),
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if stream.is_none() {
                    // Persist anything queued before the (potentially blocking) connect, so
                    // nothing is lost while we wait on the socket.
                    while let Ok(out) = rx.try_recv() {
                        spill.append(&out.to_record());
                    }
                    if let Some(mut candidate) = connect(&addr) {
                        if drain(&mut spill, &mut candidate, &formatter).is_ok() {
                            stream = Some(candidate);
                        }
                    }
                } else if !spill.is_empty() {
                    // Connected but records remain (spilled by a failed live send that has
                    // since reconnected): flush them.
                    if let Some(active) = stream.as_mut() {
                        if drain(&mut spill, active, &formatter).is_err() {
                            stream = None;
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Initialize the global `log` logger: RFC 5424 over TCP with a persistent spill buffer.
///
/// * `spill_path` — a filesystem path on a partition the caller has mounted; the file and
///   a sibling `.tmp` are created and managed here.
/// * `max_records` — cap on buffered records; the oldest are dropped in batches once full.
pub fn init_tcp_ipv4(
    hostname: Option<&'static str>,
    process: &'static str,
    facility: Facility,
    log_level: log::LevelFilter,
    addr: SocketAddr,
    spill_path: &str,
    max_records: usize,
) -> Result<()> {
    let formatter = Formatter5424 {
        facility,
        hostname: hostname.map(|s| s.to_string()),
        process: process.to_string(),
        pid: 0,
    };

    // Fix the boot epoch now, before any record is captured.
    let _ = clock::boot_epoch();

    let spill = SpillStore::open(spill_path, max_records);
    let (tx, rx) = sync_channel::<Outgoing>(CHANNEL_CAPACITY);

    std::thread::Builder::new()
        .name("syslog".into())
        // Headroom for the ESP-IDF socket + VFS/FATFS + formatting call stack.
        .stack_size(16 * 1024)
        .spawn(move || run_worker(rx, addr, formatter, spill))
        .map_err(|e| Error::Initialization(Box::new(e)))?;

    let esp_logger = EspLogger::new();
    esp_logger.set_target_level("main", log::LevelFilter::Debug).ok();

    let logger = SyslogLogger {
        sender: tx,
        esp_logger: Arc::new(Mutex::new(esp_logger)),
    };
    log::set_boxed_logger(Box::new(logger)).map_err(|e| Error::Initialization(Box::new(e)))?;
    log::set_max_level(log_level);
    Ok(())
}
