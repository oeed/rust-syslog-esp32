//! Syslog for ESP32 (UDP, RFC 5424)
//!
//! This crate provides a UDP-only, non-blocking producer logger for ESP32.
//! Messages are formatted in RFC 5424 and delivered via a background worker thread.
//!
//! Use with the `log` crate via `init_udp_ipv4`.
//!
//! Example: initialize global logger and use `log` macros
//!
//! ```rust
//! use log::LevelFilter;
//! use syslog::{init_udp_ipv4, Facility};
//!
//! fn main() {
//!     // Send RFC 5424 logs over UDP to 192.168.1.10:514
//!     let _ = init_udp_ipv4(
//!         Some("esp32"),
//!         "myprogram",
//!         Facility::LOG_USER,
//!         LevelFilter::Info,
//!         [192, 168, 1, 10],
//!         514,
//!     );
//!
//!     log::info!("hello world");
//! }
//! ```
//!
//! Example: create a dedicated UDP logger and write RFC 5424 directly
//!
//! ```rust
//! use std::collections::BTreeMap;
//! use syslog::{udp_logger_ipv4, Facility, Formatter5424, LogFormat};
//!
//! fn main() {
//!     let formatter = Formatter5424 {
//!         facility: Facility::LOG_USER,
//!         hostname: Some("esp32".to_string()),
//!         process: "myprogram".to_string(),
//!         pid: 0,
//!     };
//!
//!     // Create a UDP logger targeting 127.0.0.1:514
//!     let mut logger = udp_logger_ipv4(formatter, [127, 0, 0, 1], 514, 256)
//!         .expect("create udp logger");
//!
//!     // RFC 5424 message: (message_id, structured_data, message)
//!     let message_id = 1u32;
//!     let data: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
//!     let _ = logger.err((message_id, data, "hello world"));
//! }
//! ```
extern crate log;
extern crate time;

use std::fmt::{self, Arguments};
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{
    Arc, Mutex,
    mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
};

use esp_idf_svc::log::EspLogger;
use log::{Level, Log, Metadata, Record};

mod errors;
mod facility;
mod format;

pub use errors::*;
pub use facility::Facility;
pub use format::Severity;
pub use format::{Formatter5424, LogFormat};

pub type Priority = u8;

/// Main logging structure
pub struct Logger<Backend: Write, Formatter> {
    pub formatter: Formatter,
    pub backend: Backend,
}

impl<W: Write, F> Logger<W, F> {
    pub fn new(backend: W, formatter: F) -> Self {
        Logger { backend, formatter }
    }

    pub fn emerg<T>(&mut self, message: T) -> Result<()>
    where
        F: LogFormat<T>,
    {
        self.formatter.emerg(&mut self.backend, message)
    }

    pub fn alert<T>(&mut self, message: T) -> Result<()>
    where
        F: LogFormat<T>,
    {
        self.formatter.alert(&mut self.backend, message)
    }

    pub fn crit<T>(&mut self, message: T) -> Result<()>
    where
        F: LogFormat<T>,
    {
        self.formatter.crit(&mut self.backend, message)
    }

    pub fn err<T>(&mut self, message: T) -> Result<()>
    where
        F: LogFormat<T>,
    {
        self.formatter.err(&mut self.backend, message)
    }

    pub fn warning<T>(&mut self, message: T) -> Result<()>
    where
        F: LogFormat<T>,
    {
        self.formatter.warning(&mut self.backend, message)
    }

    pub fn notice<T>(&mut self, message: T) -> Result<()>
    where
        F: LogFormat<T>,
    {
        self.formatter.notice(&mut self.backend, message)
    }

    pub fn info<T>(&mut self, message: T) -> Result<()>
    where
        F: LogFormat<T>,
    {
        self.formatter.info(&mut self.backend, message)
    }

    pub fn debug<T>(&mut self, message: T) -> Result<()>
    where
        F: LogFormat<T>,
    {
        self.formatter.debug(&mut self.backend, message)
    }
}

/// Non-blocking queue backend for UDP worker
pub struct QueueBackend {
    sender: SyncSender<Vec<u8>>,
}

impl QueueBackend {
    fn new(remote_addr: SocketAddr, queue_capacity: usize) -> io::Result<QueueBackend> {
        let (tx, rx): (SyncSender<Vec<u8>>, Receiver<Vec<u8>>) = sync_channel(queue_capacity);
        spawn_udp_worker(remote_addr, rx)?;
        Ok(QueueBackend { sender: tx })
    }
}

impl Write for QueueBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Trim to 1024 bytes per requirement
        let len = if buf.len() > 1024 { 1024 } else { buf.len() };
        let slice = &buf[..len];
        match self.sender.try_send(slice.to_vec()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {}
        }
        Ok(len)
    }

    fn write_fmt(&mut self, args: Arguments) -> io::Result<()> {
        // Format the entire message once, then send as a single UDP datagram
        let mut s = String::new();
        let _ = fmt::write(&mut s, args);
        let bytes = s.as_bytes();
        let len = if bytes.len() > 1024 {
            1024
        } else {
            bytes.len()
        };
        match self.sender.try_send(bytes[..len].to_vec()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {}
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn spawn_udp_worker(remote_addr: SocketAddr, rx: Receiver<Vec<u8>>) -> io::Result<()> {
    // Bind ephemeral local address
    let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))?;
    std::thread::spawn(move || {
        // Dedicated thread: may block on recv/send. All errors are ignored.
        while let Ok(msg) = rx.recv() {
            let _ = socket.send_to(&msg, remote_addr);
        }
    });
    Ok(())
}

pub struct BasicLogger {
    logger: Arc<Mutex<Logger<QueueBackend, Formatter5424>>>,
    esp_logger: Arc<Mutex<EspLogger>>,
}

impl BasicLogger {
    pub fn new(logger: Logger<QueueBackend, Formatter5424>) -> BasicLogger {
        let esp_logger = EspLogger::new();
        BasicLogger {
            logger: Arc::new(Mutex::new(logger)),
            esp_logger: Arc::new(Mutex::new(esp_logger)),
        }
    }
}

#[allow(unused_variables, unused_must_use)]
impl Log for BasicLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level() && metadata.level() <= log::STATIC_MAX_LEVEL
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if let Ok(esp_logger) = self.esp_logger.try_lock() {
            esp_logger.log(record);
        }

        let message = format!("{}", record.args());
        if let Ok(mut logger) = self.logger.try_lock() {
            // RFC5424 requires (message_id, structured_data, message)
            let message_id = 1u32;
            let data = std::collections::BTreeMap::new();
            let _ = match record.level() {
                Level::Error => logger.err((message_id, data.clone(), message)),
                Level::Warn => logger.warning((message_id, data.clone(), message)),
                Level::Info => logger.info((message_id, data.clone(), message)),
                Level::Debug => logger.debug((message_id, data.clone(), message)),
                Level::Trace => logger.debug((message_id, data.clone(), message)),
            };
        } // else: drop on contention
    }

    fn flush(&self) {
        if let Ok(mut logger) = self.logger.try_lock() {
            let _ = logger.backend.flush();
        }
    }
}

/// Create a UDP RFC5424 logger targeting an IPv4 address.
pub fn udp_logger_ipv4(
    formatter: Formatter5424,
    addr: SocketAddr,
    queue_capacity: usize,
) -> Result<Logger<QueueBackend, Formatter5424>> {
    let backend =
        QueueBackend::new(addr, queue_capacity).map_err(|e| Error::Initialization(Box::new(e)))?;
    Ok(Logger::new(backend, formatter))
}

/// Initialize global logger for `log` crate, UDP IPv4 only.
pub fn init_udp_ipv4(
    hostname: Option<&'static str>,
    process: &'static str,
    facility: Facility,
    log_level: log::LevelFilter,
    addr: SocketAddr,
) -> Result<()> {
    let formatter = Formatter5424 {
        facility,
        hostname: hostname.map(|s| s.to_string()),
        process: process.to_string(),
        pid: 0,
    };
    let logger = udp_logger_ipv4(formatter, addr, 256)?;
    log::set_boxed_logger(Box::new(BasicLogger::new(logger)))
        .map_err(|e| Error::Initialization(Box::new(e)))?;
    log::set_max_level(log_level);
    Ok(())
}
