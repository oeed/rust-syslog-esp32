//! using syslog with the log crate
extern crate esp_syslog;
#[macro_use]
extern crate log;

use log::LevelFilter;
use esp_syslog::{BasicLogger, Facility, Formatter3164, TcpStream, LoggerBackend, BufWriter};

fn main() {
    let formatter = Formatter3164 {
        facility: Facility::LOG_USER,
        hostname: None,
        process: "myprogram".into(),
        pid: 0,
    };

    let tcp_server = TcpStream::connect(("127.0.0.1", 601)).map(|s| LoggerBackend::Tcp(BufWriter::new(s)));

    let logger = esp_syslog::tcp(formatter, tcp_server).expect("could not connect to syslog");
    log::set_boxed_logger(Box::new(BasicLogger::new(logger)))
        .map(|()| log::set_max_level(LevelFilter::Info))
        .expect("could not register logger");

    info!("hello world");
}
