use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use syslog::{Facility, Formatter5424, LogFormat, udp_logger_ipv4};

fn main() {
    let formatter = Formatter5424 {
        facility: Facility::LOG_USER,
        hostname: Some("esp32".to_string()),
        process: "myprogram".into(),
        pid: 0,
    };

    udp_logger_ipv4(
        formatter,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 514),
        256,
    )
    .expect("could not create udp logger");

    writer
        .err((1, BTreeMap::new(), "hello world"))
        .expect("could not write error message");
}
