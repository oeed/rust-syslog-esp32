## Syslog for ESP32 (UDP, RFC5424)

This fork targets ESP32 only. It sends RFC 5424-formatted messages over UDP using a background worker thread. The producer side never blocks; messages are dropped if the queue is full or if any error occurs.

### Install

```toml
[dependencies]
syslog = { path = "." }
log = "0.4"
``

### Use with the `log` crate

```rust
use log::LevelFilter;
use syslog::{init_udp_ipv4, Facility};

fn main() {
    let _ = init_udp_ipv4(
        Some("esp32"),
        "myprogram",
        Facility::LOG_USER,
        LevelFilter::Info,
        [192, 168, 1, 10],
        514,
    );

    log::info!("hello world");
}
```

### Direct RFC5424 usage

```rust
use std::collections::BTreeMap;
use syslog::{udp_logger_ipv4, Facility, Formatter5424, LogFormat};

fn main() {
    let formatter = Formatter5424 {
        facility: Facility::LOG_USER,
        hostname: Some("esp32".to_string()),
        process: "myprogram".to_string(),
        pid: 0,
    };

    let mut logger = udp_logger_ipv4(formatter, [127, 0, 0, 1], 514, 256).unwrap();
    let message_id = 1u32;
    let data: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let _ = logger.err((message_id, data, "hello world"));
}
```
