//! Time handling for log timestamps on a device with no battery-backed RTC.
//!
//! The ESP32 has no wall-clock across a power loss, so absolute time is only known
//! once something (e.g. SNTP) supplies it. We therefore timestamp every event with a
//! monotonic uptime at capture, and convert to wall-clock lazily once an anchor — a
//! single `(uptime, wall-clock)` pairing — is established via [`sync_wall_clock`].
//!
//! Each captured record also carries a per-boot [`boot_epoch`]. The uptime → wall-clock
//! conversion is only valid within the boot that captured it, so a record recovered from
//! flash after a reboot is only resolved if its epoch matches the current one; otherwise
//! its time is genuinely unknown and is reported as the RFC 5424 NILVALUE.

use std::sync::{Mutex, OnceLock};

use esp_idf_svc::sys::{esp_random, esp_timer_get_time};

static BOOT_EPOCH: OnceLock<u64> = OnceLock::new();

// A Mutex (rather than an AtomicU64 pair) because the xtensa ESP32 target has no native
// 64-bit atomics. Reads happen per formatted log line, so the lock is uncontended in practice.
static ANCHOR: Mutex<Option<Anchor>> = Mutex::new(None);

#[derive(Clone, Copy)]
struct Anchor {
    uptime_us: u64,
    wall_us: u64,
}

/// A value unique to this boot, used to tell whether a record recovered from flash was
/// captured in the current boot (and so can be resolved against the current anchor).
pub fn boot_epoch() -> u64 {
    *BOOT_EPOCH.get_or_init(|| {
        let hi = unsafe { esp_random() } as u64;
        let lo = unsafe { esp_random() } as u64;
        (hi << 32) | lo
    })
}

/// Microseconds since boot, from the ESP monotonic timer.
pub fn uptime_micros() -> u64 {
    (unsafe { esp_timer_get_time() }).max(0) as u64
}

/// Record the current `(uptime, wall-clock)` pairing. Call this whenever a trusted
/// wall-clock becomes available (e.g. on each SNTP sync); the most recent pairing wins.
pub fn sync_wall_clock(wall_unix_micros: u64) {
    let anchor = Anchor {
        uptime_us: uptime_micros(),
        wall_us: wall_unix_micros,
    };
    if let Ok(mut guard) = ANCHOR.lock() {
        *guard = Some(anchor);
    }
}

/// Resolve a capture-time uptime (from the current boot) to absolute Unix microseconds,
/// or `None` if no anchor has been established yet.
pub fn resolve_wall_micros(uptime_micros: u64) -> Option<u64> {
    let anchor = (*ANCHOR.lock().ok()?)?;
    let wall = anchor.wall_us as i64 + (uptime_micros as i64 - anchor.uptime_us as i64);
    if wall < 0 { None } else { Some(wall as u64) }
}
