//! Syslog
//!
//! This crate provides facilities to send log messages via syslog.
//! It supports UDP and TCP for remote servers.
//!
//! Messages can be passed directly without modification, or in RFC 3164 or RFC 5424 format
//!
//! The code is available on [Github](https://github.com/Geal/rust-syslog)
//!
//! # Example
//!
//! ```rust
//! use esp_syslog::{Facility, Formatter3164, TcpStream, LoggerBackend, BufWriter};
//!
//! let formatter = Formatter3164 {
//!     facility: Facility::LOG_USER,
//!     hostname: "esp32-mydeviceid".into(),
//!     process: "myprogram".into(),
//!     pid: 0,
//! };
//!
//! let tcp_server = TcpStream::connect(("127.0.0.1", 601)).map(|s| LoggerBackend::Tcp(BufWriter::new(s)));
//!
//! match esp_syslog::tcp(formatter, tcp_server) {
//!     Err(e) => println!("impossible to connect to syslog: {:?}", e),
//!     Ok(mut writer) => {
//!         writer.err("hello world").expect("could not write error message");
//!     }
//! }
//! ```
//!
//! It can be used directly with the log crate as follows:
//!
//! ```rust
//! extern crate log;
//!
//! use esp_syslog::{Facility, Formatter3164, BasicLogger, TcpStream, LoggerBackend, BufWriter};
//! use log::{SetLoggerError, LevelFilter, info};
//!
//! let formatter = Formatter3164 {
//!     facility: Facility::LOG_USER,
//!     hostname: "esp32-mydeviceid".into(),
//!     process: "myprogram".into(),
//!     pid: 0,
//! };
//!
//! let tcp_server = TcpStream::connect(("127.0.0.1", 601)).map(|s| LoggerBackend::Tcp(BufWriter::new(s)));
//!
//! let logger = match esp_syslog::tcp(formatter, tcp_server) {
//!     Err(e) => { println!("impossible to connect to syslog: {:?}", e); return; },
//!     Ok(logger) => logger,
//! };
//! log::set_boxed_logger(Box::new(BasicLogger::new(logger)))
//!         .map(|()| log::set_max_level(LevelFilter::Info));
//!
//! info!("hello world");
//!
#![crate_type = "lib"]

#[macro_use]
extern crate error_chain;
extern crate log;
extern crate time;

use std::fmt::{self, Arguments};
use std::io::{self, BufWriter, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex};

use log::{Level, Log, Metadata, Record};

mod errors;
mod facility;
mod format;
pub use errors::*;
pub use facility::Facility;
pub use format::Severity;

pub use format::{Formatter3164, Formatter5424, LogFormat};

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

pub enum LoggerBackend {
    Udp(UdpSocket, SocketAddr),
    Tcp(BufWriter<TcpStream>),
}

impl Write for LoggerBackend {
    /// Sends a message directly, without any formatting
    fn write(&mut self, message: &[u8]) -> io::Result<usize> {
        match *self {
            LoggerBackend::Udp(ref socket, ref addr) => socket.send_to(message, addr),
            LoggerBackend::Tcp(ref mut socket) => socket.write(message),
        }
    }

    fn write_fmt(&mut self, args: Arguments) -> io::Result<()> {
        match *self {
            LoggerBackend::Udp(ref socket, ref addr) => {
                let message = fmt::format(args);
                socket.send_to(message.as_bytes(), addr).map(|_| ())
            }
            LoggerBackend::Tcp(ref mut socket) => socket.write_fmt(args),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match *self {
            LoggerBackend::Udp(_, _) => Ok(()),
            LoggerBackend::Tcp(ref mut socket) => socket.flush(),
        }
    }
}

/// returns a UDP logger connecting `local` and `server`
pub fn udp<T: ToSocketAddrs, F>(
    formatter: F,
    local: T,
    server: T,
) -> Result<Logger<LoggerBackend, F>> {
    server
        .to_socket_addrs()
        .chain_err(|| ErrorKind::Initialization)
        .and_then(|mut server_addr_opt| {
            server_addr_opt
                .next()
                .chain_err(|| ErrorKind::Initialization)
        })
        .and_then(|server_addr| {
            UdpSocket::bind(local)
                .chain_err(|| ErrorKind::Initialization)
                .map(|socket| Logger {
                    formatter,
                    backend: LoggerBackend::Udp(socket, server_addr),
                })
        })
}

/// returns a TCP logger connecting `local` and `server`
pub fn tcp<T: ToSocketAddrs, F>(formatter: F, server: T) -> Result<Logger<LoggerBackend, F>> {
    TcpStream::connect(server)
        .chain_err(|| ErrorKind::Initialization)
        .map(|socket| Logger {
            formatter,
            backend: LoggerBackend::Tcp(BufWriter::new(socket)),
        })
}

#[derive(Clone)]
pub struct BasicLogger {
    logger: Arc<Mutex<Logger<LoggerBackend, Formatter3164>>>,
}

impl BasicLogger {
    pub fn new(logger: Logger<LoggerBackend, Formatter3164>) -> BasicLogger {
        BasicLogger {
            logger: Arc::new(Mutex::new(logger)),
        }
    }
}

#[allow(unused_variables, unused_must_use)]
impl Log for BasicLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level() && metadata.level() <= log::STATIC_MAX_LEVEL
    }

    fn log(&self, record: &Record) {
        //FIXME: temporary patch to compile
        let message = format!("{}", record.args());
        let mut logger = self.logger.lock().unwrap();
        match record.level() {
            Level::Error => logger.err(message),
            Level::Warn => logger.warning(message),
            Level::Info => logger.info(message),
            Level::Debug => logger.debug(message),
            Level::Trace => logger.debug(message),
        };
    }

    fn flush(&self) {
        let _ = self.logger.lock().unwrap().backend.flush();
    }
}

/// UDP Logger init function compatible with log crate
pub fn init_udp<T: ToSocketAddrs>(
    local: T,
    server: T,
    hostname: String,
    facility: Facility,
    log_level: log::LevelFilter,
    process: String,
    pid: u32,
) -> Result<()> {
    let formatter = Formatter3164 {
        facility,
        hostname: Some(hostname),
        process,
        pid,
    };
    let logger = udp(formatter, local, server).unwrap();
    let basic_logger = Box::new(BasicLogger::new(logger));
    log::set_logger(Box::leak(basic_logger)).chain_err(|| ErrorKind::Initialization)?;

    log::set_max_level(log_level);
    Ok(())
}

/// TCP Logger init function compatible with log crate
pub fn init_tcp<T: ToSocketAddrs>(
    server: T,
    hostname: String,
    facility: Facility,
    log_level: log::LevelFilter,
    process: String,
    pid: u32,
) -> Result<()> {
    let formatter = Formatter3164 {
        facility,
        hostname: Some(hostname),
        process,
        pid,
    };

    let logger = tcp(formatter, server).unwrap();
    let basic_logger = Box::new(BasicLogger::new(logger));
    log::set_logger(Box::leak(basic_logger)).chain_err(|| ErrorKind::Initialization)?;

    log::set_max_level(log_level);
    Ok(())
}
