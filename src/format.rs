use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::Write;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::Priority;
use crate::errors::*;
use crate::facility::Facility;

#[allow(non_camel_case_types)]
#[derive(Copy, Clone)]
pub enum Severity {
    LOG_EMERG,
    LOG_ALERT,
    LOG_CRIT,
    LOG_ERR,
    LOG_WARNING,
    LOG_NOTICE,
    LOG_INFO,
    LOG_DEBUG,
}

impl Severity {
    /// Reconstruct a severity from its syslog numeric value (0..=7). Out-of-range
    /// values fall back to `LOG_DEBUG`.
    pub fn from_u8(value: u8) -> Severity {
        match value {
            0 => Severity::LOG_EMERG,
            1 => Severity::LOG_ALERT,
            2 => Severity::LOG_CRIT,
            3 => Severity::LOG_ERR,
            4 => Severity::LOG_WARNING,
            5 => Severity::LOG_NOTICE,
            6 => Severity::LOG_INFO,
            _ => Severity::LOG_DEBUG,
        }
    }
}

pub trait LogFormat<T> {
    /// Format a single message. `timestamp` is the moment the event occurred; pass
    /// `None` when it is unknown, which emits the RFC 5424 NILVALUE (`-`) in the
    /// TIMESTAMP field rather than stamping the time of formatting.
    fn format<W: Write>(
        &self,
        w: &mut W,
        severity: Severity,
        message: T,
        timestamp: Option<OffsetDateTime>,
    ) -> Result<()>;
}

/// RFC 5424 structured data
pub type StructuredData = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Clone, Debug)]
pub struct Formatter5424 {
    pub facility: Facility,
    pub hostname: Option<String>,
    pub process: String,
    pub pid: u32,
}

impl Formatter5424 {
    pub fn format_5424_structured_data(&self, data: StructuredData) -> String {
        if data.is_empty() {
            "-".to_string()
        } else {
            let mut res = String::new();
            for (id, params) in &data {
                res = res + "[" + id;
                for (name, value) in params {
                    res =
                        res + " " + name + "=\"" + &escape_structure_data_param_value(value) + "\"";
                }
                res += "]";
            }

            res
        }
    }
}

impl<T: Display> LogFormat<(u32, StructuredData, T)> for Formatter5424 {
    fn format<W: Write>(
        &self,
        w: &mut W,
        severity: Severity,
        log_message: (u32, StructuredData, T),
        timestamp: Option<OffsetDateTime>,
    ) -> Result<()> {
        let (message_id, data, message) = log_message;

        let ts_string = match timestamp {
            Some(timestamp) => {
                // Guard against sub-second precision over 6 digits per rfc5424 section 6
                let timestamp = timestamp
                    .replace_nanosecond(timestamp.nanosecond() / 1000 * 1000)
                    .unwrap_or(timestamp);
                timestamp.format(&Rfc3339).unwrap_or_else(|_| "-".to_string())
            }
            // Time is not known (e.g. logged before the clock was synced, or recovered
            // from a previous boot): emit the RFC 5424 NILVALUE.
            None => "-".to_string(),
        };

        write!(
            w,
            "<{}>1 {} {} {} {} {} {} {}", // v1
            encode_priority(severity, self.facility),
            ts_string,
            self.hostname
                .as_ref()
                .map(|x| &x[..])
                .unwrap_or("localhost"),
            self.process,
            self.pid,
            message_id,
            self.format_5424_structured_data(data),
            message
        )
        .map_err(Error::Write)
    }
}

impl Default for Formatter5424 {
    /// Returns a `Formatter5424` with default settings.
    ///
    /// The default settings are as follows:
    ///
    /// * `facility`: `LOG_USER`, as [specified by POSIX].
    /// * `hostname`: `None`.
    /// * `process`: `"esp32"`.
    /// * `pid`: `0`.
    ///
    /// [specified by POSIX]: https://pubs.opengroup.org/onlinepubs/9699919799/functions/closelog.html
    fn default() -> Self {
        Self {
            facility: Facility::LOG_USER,
            hostname: None,
            process: "esp32".to_string(),
            pid: 0,
        }
    }
}

fn escape_structure_data_param_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(']', "\\]")
}

fn encode_priority(severity: Severity, facility: Facility) -> Priority {
    facility as u8 | severity as u8
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn backslash_is_escaped() {
        let string = "\\";
        let value = escape_structure_data_param_value(string);
        assert_eq!(value, "\\\\");
    }
    #[test]
    fn quote_is_escaped() {
        let string = "foo\"bar";
        let value = escape_structure_data_param_value(string);
        assert_eq!(value, "foo\\\"bar");
    }
    #[test]
    fn end_bracket_is_escaped() {
        let string = "]";
        let value = escape_structure_data_param_value(string);
        assert_eq!(value, "\\]");
    }

    #[test]
    fn test_formatter5424_defaults() {
        let d = Formatter5424::default();

        // `Facility` doesn't implement `PartialEq`, so we use a `match` instead.
        assert!(match d.facility {
            Facility::LOG_USER => true,
            _ => false,
        });

        // hostname default is None on ESP32
        assert!(d.hostname.is_none());

        assert_eq!(d.process, "esp32");
    }

    #[test]
    fn missing_timestamp_is_nilvalue() {
        let formatter = Formatter5424::default();
        let mut buf: Vec<u8> = Vec::new();
        let data: StructuredData = BTreeMap::new();
        formatter
            .format(&mut buf, Severity::LOG_INFO, (1u32, data, "hi"), None)
            .unwrap();
        let line = String::from_utf8(buf).unwrap();
        // The TIMESTAMP field (between version `1` and the hostname) must be the NILVALUE.
        assert!(line.contains(">1 - "), "expected NILVALUE timestamp, got: {line}");
    }
}
