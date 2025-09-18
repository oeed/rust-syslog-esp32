use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::Write;
use time;

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

pub trait LogFormat<T> {
    fn format<W: Write>(&self, w: &mut W, severity: Severity, message: T) -> Result<()>;

    fn emerg<W: Write>(&mut self, w: &mut W, message: T) -> Result<()> {
        self.format(w, Severity::LOG_EMERG, message)
    }

    fn alert<W: Write>(&mut self, w: &mut W, message: T) -> Result<()> {
        self.format(w, Severity::LOG_ALERT, message)
    }

    fn crit<W: Write>(&mut self, w: &mut W, message: T) -> Result<()> {
        self.format(w, Severity::LOG_CRIT, message)
    }

    fn err<W: Write>(&mut self, w: &mut W, message: T) -> Result<()> {
        self.format(w, Severity::LOG_ERR, message)
    }

    fn warning<W: Write>(&mut self, w: &mut W, message: T) -> Result<()> {
        self.format(w, Severity::LOG_WARNING, message)
    }

    fn notice<W: Write>(&mut self, w: &mut W, message: T) -> Result<()> {
        self.format(w, Severity::LOG_NOTICE, message)
    }

    fn info<W: Write>(&mut self, w: &mut W, message: T) -> Result<()> {
        self.format(w, Severity::LOG_INFO, message)
    }

    fn debug<W: Write>(&mut self, w: &mut W, message: T) -> Result<()> {
        self.format(w, Severity::LOG_DEBUG, message)
    }
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
    ) -> Result<()> {
        let (message_id, data, message) = log_message;

        // Guard against sub-second precision over 6 digits per rfc5424 section 6
        let timestamp_now = time::OffsetDateTime::now_utc();
        // Removing significant figures beyond 6 digits
        let timestamp = timestamp_now
            .replace_nanosecond(timestamp_now.nanosecond() / 1000 * 1000)
            .unwrap_or(timestamp_now);

        let ts_string = match timestamp.format(&time::format_description::well_known::Rfc3339) {
            Ok(s) => s,
            Err(_) => "-".to_string(),
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
    /// * `hostname`: Automatically detected using [the `hostname` crate], if possible.
    /// * `process`: Automatically detected using [`std::env::current_exe`], or if that fails, an empty string.
    /// * `pid`: Automatically detected using [`libc::getpid`].
    ///
    /// [`libc::getpid`]: https://docs.rs/libc/0.2/libc/fn.getpid.html
    /// [specified by POSIX]: https://pubs.opengroup.org/onlinepubs/9699919799/functions/closelog.html
    /// [`std::env::current_exe`]: https://doc.rust-lang.org/std/env/fn.current_exe.html
    /// [the `hostname` crate]: https://crates.io/crates/hostname
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
}
