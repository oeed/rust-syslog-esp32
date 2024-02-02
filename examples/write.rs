extern crate esp_syslog;

use esp_syslog::{Facility, Formatter3164, TcpStream, LoggerBackend, BufWriter};

fn main() {
    let formatter = Formatter3164 {
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
                .err("hello world")
                .expect("could not write error message");
            writer
                .err("hello all".to_string())
                .expect("could not write error message");
        }
    }
}
