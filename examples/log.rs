use log::LevelFilter;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use syslog::{Facility, init_udp_ipv4};

fn main() {
    init_udp_ipv4(
        Some("esp32"),
        "myprogram",
        Facility::LOG_USER,
        LevelFilter::Info,
        [127, 0, 0, 1],
        514,
    )
    .expect("could not register logger");

    info!("hello world");
}
