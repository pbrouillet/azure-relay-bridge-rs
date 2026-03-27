//! Logging setup for azbridge, mapping C#-compatible log levels to `tracing`.
//!
//! Log levels from the upstream C# azbridge config:
//! - `QUIET` / `NONE` — no output
//! - `FATAL` — only fatal/error-level events
//! - `ERROR` — errors
//! - `INFO` — informational (default)
//! - `VERBOSE` — debug-level detail
//! - `DEBUG` / `DEBUG1` / `DEBUG2` / `DEBUG3` — trace-level detail

use std::path::Path;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt;

/// Maps a C#-compatible log level string to a `tracing` max level.
pub fn parse_log_level(level: &str) -> Level {
    match level.to_uppercase().as_str() {
        "QUIET" | "NONE" => Level::ERROR, // tracing has no "off"; we filter via EnvFilter
        "FATAL" | "ERROR" => Level::ERROR,
        "INFO" => Level::INFO,
        "VERBOSE" => Level::DEBUG,
        "DEBUG" | "DEBUG1" | "DEBUG2" | "DEBUG3" => Level::TRACE,
        _ => Level::INFO,
    }
}

/// Returns true if the log level string means "suppress all output".
pub fn is_quiet(level: &str) -> bool {
    matches!(level.to_uppercase().as_str(), "QUIET" | "NONE")
}

/// Initialize the tracing subscriber.
///
/// Returns an optional `WorkerGuard` that must be kept alive for the
/// duration of the program when logging to a file (dropping it flushes
/// and closes the log file).
pub fn init(
    log_level: Option<&str>,
    log_file: Option<&Path>,
) -> Option<WorkerGuard> {
    let level = log_level.map(parse_log_level).unwrap_or(Level::INFO);
    let quiet = log_level.is_some_and(is_quiet);

    if quiet && log_file.is_none() {
        // No output at all — install a no-op subscriber
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(Level::ERROR)
            .with_writer(std::io::sink)
            .finish();
        tracing::subscriber::set_global_default(subscriber).ok();
        return None;
    }

    if let Some(path) = log_file {
        // File logging with non-blocking writer
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let filename = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("azbridge.log");

        let file_appender = tracing_appender::rolling::never(dir, filename);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

        let subscriber = fmt()
            .with_max_level(level)
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_target(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).ok();
        Some(guard)
    } else {
        // Console logging
        let subscriber = fmt()
            .with_max_level(level)
            .with_target(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).ok();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_log_level_quiet() {
        assert_eq!(parse_log_level("QUIET"), Level::ERROR);
        assert_eq!(parse_log_level("NONE"), Level::ERROR);
        assert_eq!(parse_log_level("quiet"), Level::ERROR);
    }

    #[test]
    fn parse_log_level_error() {
        assert_eq!(parse_log_level("ERROR"), Level::ERROR);
        assert_eq!(parse_log_level("FATAL"), Level::ERROR);
    }

    #[test]
    fn parse_log_level_info() {
        assert_eq!(parse_log_level("INFO"), Level::INFO);
    }

    #[test]
    fn parse_log_level_verbose() {
        assert_eq!(parse_log_level("VERBOSE"), Level::DEBUG);
    }

    #[test]
    fn parse_log_level_debug_variants() {
        assert_eq!(parse_log_level("DEBUG"), Level::TRACE);
        assert_eq!(parse_log_level("DEBUG1"), Level::TRACE);
        assert_eq!(parse_log_level("DEBUG2"), Level::TRACE);
        assert_eq!(parse_log_level("DEBUG3"), Level::TRACE);
    }

    #[test]
    fn parse_log_level_unknown_defaults_to_info() {
        assert_eq!(parse_log_level("GARBAGE"), Level::INFO);
        assert_eq!(parse_log_level(""), Level::INFO);
    }

    #[test]
    fn is_quiet_checks() {
        assert!(is_quiet("QUIET"));
        assert!(is_quiet("NONE"));
        assert!(is_quiet("quiet"));
        assert!(!is_quiet("INFO"));
        assert!(!is_quiet("ERROR"));
    }
}
