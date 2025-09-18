use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use syslog::{Facility, Formatter5424, LogFormat, udp_logger_ipv4};

fn main() {
    let formatter = Formatter5424 {
        facility: Facility::LOG_USER,
        hostname: Some("esp32".to_string()),
        process: "myprogram".to_string(),
        pid: 0,
    };

    let mut writer = udp_logger_ipv4(
        formatter,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 514),
        256,
    )
    .expect("could not create udp logger");

    // RFC5424: (message_id, structured_data, message)
    let message_id = 1u32;
    let data: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let _ = writer.err((message_id, data, "hello world"));
}
