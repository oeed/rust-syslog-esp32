extern crate syslog;

use std::collections::BTreeMap;
use syslog::{udp_logger_ipv4, Facility, Formatter5424, LogFormat};

fn main() {
    let formatter = Formatter5424 {
        facility: Facility::LOG_USER,
        hostname: Some("esp32".to_string()),
        process: "myprogram".into(),
        pid: 0,
    };

    let mut writer =
        udp_logger_ipv4(formatter, [127, 0, 0, 1], 514, 256).expect("could not create udp logger");

    writer
        .err((1, BTreeMap::new(), "hello world"))
        .expect("could not write error message");
}
