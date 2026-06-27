use log::LevelFilter;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use syslog_esp32::{Facility, init_tcp_ipv4};

fn main() {
    init_tcp_ipv4(
        Some("esp32"),
        "myprogram",
        Facility::LOG_USER,
        LevelFilter::Info,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 514),
        // A path on a filesystem the firmware has mounted (e.g. a FAT flash partition).
        "/spill/log.bin",
        500,
    )
    .expect("could not register logger");

    log::info!("hello world");
}
