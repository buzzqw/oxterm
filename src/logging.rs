use std::sync::OnceLock;

use once_cell::sync::Lazy;

/// Minimal JSON-lines logger used by Oxterm.
///
/// Events are emitted on stderr as single-line JSON objects:
/// `{"timestamp": "...", "level": "INFO", "logger": "...", "event": "..."}`.
pub struct Logger;

impl Logger {
    pub fn log(level: &str, logger: &str, event: &str, exception: Option<&str>) {
        // serde_json produces valid JSON escapes (`{:?}` would emit Rust
        // escapes such as `\u{1b}`, which JSON parsers reject).
        let mut record = serde_json::Map::new();
        record.insert("timestamp".into(), utc_iso_now().into());
        record.insert("level".into(), level.into());
        record.insert("logger".into(), logger.into());
        record.insert("event".into(), event.into());
        if let Some(exc) = exception {
            record.insert("exception".into(), exc.into());
        }
        eprintln!("{}", serde_json::Value::Object(record));
    }

    pub fn error(logger: &str, event: &str) {
        Self::log("ERROR", logger, event, None);
    }

    pub fn warning(logger: &str, event: &str) {
        Self::log("WARNING", logger, event, None);
    }
}

static LOGGER_NAME: OnceLock<&'static str> = OnceLock::new();

pub fn configure_logging() {
    let _ = LOGGER_NAME.set("Oxterm");
}

#[inline]
pub fn log_warning(event: &str) {
    Logger::warning(logger_name(), event);
}

fn logger_name() -> &'static str {
    LOGGER_NAME.get().copied().unwrap_or("Oxterm")
}

/// RFC3339 UTC timestamp without external dependencies (libc `gmtime_r`).
pub fn utc_iso_now() -> String {
    use std::mem::MaybeUninit;

    let mut tv = MaybeUninit::<libc::timeval>::uninit();
    unsafe {
        libc::gettimeofday(tv.as_mut_ptr(), std::ptr::null_mut());
    }
    let tv = unsafe { tv.assume_init() };
    let mut tm = MaybeUninit::<libc::tm>::uninit();
    unsafe {
        libc::gmtime_r(&tv.tv_sec as *const libc::time_t, tm.as_mut_ptr());
    }
    let tm = unsafe { tm.assume_init() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        tv.tv_usec
    )
}

/// Local-time human timestamp (`YYYY-MM-DD HH:MM:SS`) without external
/// dependencies (libc `localtime_r`). Used for note entries, where the user's
/// wall-clock time is more meaningful than UTC.
pub fn local_human_now() -> String {
    use std::mem::MaybeUninit;

    let mut tv = MaybeUninit::<libc::timeval>::uninit();
    unsafe {
        libc::gettimeofday(tv.as_mut_ptr(), std::ptr::null_mut());
    }
    let tv = unsafe { tv.assume_init() };
    let mut tm = MaybeUninit::<libc::tm>::uninit();
    let tm = unsafe {
        if libc::localtime_r(&tv.tv_sec as *const libc::time_t, tm.as_mut_ptr()).is_null() {
            // Extremely unlikely; fall back to the UTC formatter.
            return utc_iso_now();
        }
        tm.assume_init()
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// Reusable lazy logger name for module-scoped logging.
pub fn scoped(name: &'static str) -> ScopedLogger {
    ScopedLogger { name }
}

pub struct ScopedLogger {
    name: &'static str,
}

impl ScopedLogger {
    pub fn error(&self, event: &str) {
        Logger::error(self.name, event);
    }
    pub fn warning(&self, event: &str) {
        Logger::warning(self.name, event);
    }
}

/// Static used by sub-modules to keep a single logger handle alive.
pub static LOGGER: Lazy<ScopedLogger> = Lazy::new(|| scoped("Oxterm"));
