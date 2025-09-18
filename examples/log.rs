//! using syslog UDP with the log crate
extern crate syslog;
#[macro_use]
extern crate log;

use log::LevelFilter;
use syslog::{init_udp_ipv4, Facility};

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
