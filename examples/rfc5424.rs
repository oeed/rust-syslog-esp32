extern crate esp_syslog;

use std::collections::HashMap;
use esp_syslog::{Facility, Formatter5424, TcpStream, LoggerBackend, BufWriter};

fn main() {
    let formatter = Formatter5424 {
        facility: Facility::LOG_USER,
        hostname: None,
        process: "myprogram".into(),
        pid: 0,
    };

    let tcp_server = TcpStream::connect(("127.0.0.1", 601)).map(|s| LoggerBackend::Tcp(BufWriter::new(s)));

    match esp_syslog::tcp(formatter, tcp_server) {
        Err(e) => println!("impossible to connect to syslog: {:?}", e),
        Ok(mut writer) => {
            writer
                .err((1, HashMap::new(), "hello world"))
                .expect("could not write error message");
        }
    }
}
