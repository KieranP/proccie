//! Colored, multiplexed log output for child processes: each line is prefixed
//! with a color-coded name and serialized through a mutex so lines never interleave.

use std::io::{LineWriter, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anstyle::{AnsiColor, Style};

/// Maximum bytes buffered per writer before a forced flush, bounding memory
/// for processes that emit large output without newlines.
pub const MAX_LINE_BUFFER: usize = 1024 * 1024; // 1 MiB

/// Base name for proccie's own log lines; the level is appended (e.g.
/// `system [INFO]`), alongside the process prefixes.
const SYSTEM_PREFIX: &str = "system";

/// Severity of a proccie system log line. A line is emitted only when its level
/// is at or above the `Mux`'s configured threshold; ordering follows the
/// declaration order (`Debug` < `Info` < `Warn` < `Error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Every level, lowest to highest.
    pub const ALL: [LogLevel; 4] = [
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
    ];

    /// The uppercase label shown in the log prefix (e.g. `INFO`).
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    /// The console style for lines at this level.
    fn style(self) -> Style {
        match self {
            LogLevel::Debug | LogLevel::Info => Style::new().dimmed(),
            LogLevel::Warn => Style::new().fg_color(Some(AnsiColor::Yellow.into())),
            LogLevel::Error => Style::new().fg_color(Some(AnsiColor::Red.into())).bold(),
        }
    }
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warn" | "warning" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            _ => Err(format!(
                "invalid log level '{s}' (expected one of: debug, info, warn, error)"
            )),
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// The full system prefix for `level`, e.g. `system [WARN]`.
fn system_tag(level: LogLevel) -> String {
    format!("{SYSTEM_PREFIX} [{}]", level.label())
}

/// Colors cycled through for process prefixes.
const PREFIX_COLORS: &[AnsiColor] = &[
    AnsiColor::Cyan,
    AnsiColor::Yellow,
    AnsiColor::Magenta,
    AnsiColor::Green,
    AnsiColor::Blue,
    AnsiColor::Red,
    AnsiColor::BrightCyan,
    AnsiColor::BrightYellow,
    AnsiColor::BrightMagenta,
    AnsiColor::BrightGreen,
];

/// A log multiplexer that prefixes each line with a colored process name.
pub struct Mux {
    out: Mutex<Box<dyn Write + Send>>,
    color_idx: AtomicUsize,
    pad_width: usize,
    level: LogLevel,
}

impl Mux {
    /// Width that fits every process name prefix plus the widest system prefix
    /// (`system [DEBUG]`), so process and system lines align.
    pub fn prefix_width<'a>(names: impl IntoIterator<Item = &'a str>) -> usize {
        let widest_system = LogLevel::ALL
            .iter()
            .map(|level| system_tag(*level).chars().count())
            .max()
            .unwrap_or(0);
        names
            .into_iter()
            // `format!` pads by char count, so measure in chars, not bytes.
            .map(|name| name.chars().count())
            .chain(std::iter::once(widest_system))
            .max()
            .unwrap_or(0)
    }

    /// Creates a `Mux` writing to `out`, padding name prefixes to `pad_width`.
    /// System lines below `level` are suppressed.
    pub fn new(out: impl Write + Send + 'static, pad_width: usize, level: LogLevel) -> Arc<Mux> {
        Arc::new(Mux {
            // Every emitted batch ends in '\n', so LineWriter passes it
            // through to `out` as a single write.
            out: Mutex::new(Box::new(LineWriter::new(out))),
            color_idx: AtomicUsize::new(0),
            pad_width,
            level,
        })
    }

    /// Returns the log sink for a process: lines are prefixed with `name` in a
    /// unique color, and optionally copied uncolored to `log_file`.
    pub fn prefix_writer(
        self: &Arc<Self>,
        name: &str,
        log_file: Option<Box<dyn Write + Send>>,
    ) -> Arc<PrefixWriter> {
        let idx = self.color_idx.fetch_add(1, Ordering::Relaxed);
        let color = PREFIX_COLORS[idx % PREFIX_COLORS.len()];
        let style = Style::new().fg_color(Some(color.into()));
        let padded = pad(name, self.pad_width);

        Arc::new(PrefixWriter {
            mux: Arc::clone(self),
            prefix: format!("{}{padded}{} | ", style.render(), style.render_reset()),
            plain_prefix: format!("{padded} | "),
            log_file: log_file.map(Mutex::new),
        })
    }

    /// Writes a system-level message tagged `system [LEVEL]` and styled by
    /// severity, only when `level` meets the configured threshold.
    pub fn log(&self, level: LogLevel, msg: impl AsRef<str>) {
        if level < self.level {
            return;
        }

        let style = level.style();
        let padded = pad(&system_tag(level), self.pad_width);

        let mut out = self.out.lock().unwrap();
        for line in msg.as_ref().trim_end_matches('\n').split('\n') {
            let _ = writeln!(
                out,
                "{open}{padded}{reset} | {open}{line}{reset}",
                open = style.render(),
                reset = style.render_reset(),
            );
        }
    }

    /// Logs at [`LogLevel::Debug`].
    pub fn debug(&self, msg: impl AsRef<str>) {
        self.log(LogLevel::Debug, msg);
    }

    /// Logs at [`LogLevel::Info`].
    pub fn info(&self, msg: impl AsRef<str>) {
        self.log(LogLevel::Info, msg);
    }

    /// Logs at [`LogLevel::Warn`].
    pub fn warn(&self, msg: impl AsRef<str>) {
        self.log(LogLevel::Warn, msg);
    }

    /// Logs at [`LogLevel::Error`].
    pub fn error(&self, msg: impl AsRef<str>) {
        self.log(LogLevel::Error, msg);
    }
}

/// The shared log sink for one process: colored console prefix, plain log-file
/// prefix, and the optional log file. Created via [`Mux::prefix_writer`].
pub struct PrefixWriter {
    mux: Arc<Mux>,
    prefix: String,
    plain_prefix: String,
    log_file: Option<Mutex<Box<dyn Write + Send>>>,
}

impl PrefixWriter {
    /// Returns a line-buffered writer for one output stream. Each stream gets its
    /// own buffer so a partial write on one never fuses into another's line.
    pub fn stream(self: &Arc<Self>) -> StreamWriter {
        StreamWriter {
            writer: Arc::clone(self),
            buf: Mutex::new(Vec::new()),
        }
    }

    /// Writes prefixed lines to the console and, if configured, the plain-text
    /// log file, batched into a single write per sink.
    fn emit<'a>(&self, lines: impl Iterator<Item = &'a [u8]> + Clone) {
        let _ = self
            .mux
            .out
            .lock()
            .unwrap()
            .write_all(&join_lines(&self.prefix, lines.clone()));

        if let Some(log_file) = &self.log_file {
            let _ = log_file
                .lock()
                .unwrap()
                .write_all(&join_lines(&self.plain_prefix, lines));
        }
    }
}

/// A line-buffered writer for one output stream, sharing its parent
/// [`PrefixWriter`]'s prefix, log file, and output lock.
pub struct StreamWriter {
    writer: Arc<PrefixWriter>,
    buf: Mutex<Vec<u8>>,
}

impl StreamWriter {
    /// Appends `data`, emitting each completed line with the process prefix.
    /// Output is held until a newline, or force-flushed past [`MAX_LINE_BUFFER`].
    pub fn write(&self, data: &[u8]) {
        let mut buf = self.buf.lock().unwrap();
        // Held bytes are always newline-free (every path below drains through
        // the last newline), so only the new chunk needs scanning.
        let held = buf.len();
        buf.extend_from_slice(data);

        if let Some(last_newline) = data.iter().rposition(|&b| b == b'\n').map(|i| held + i) {
            self.writer.emit(buf[..last_newline].split(|&b| b == b'\n'));
            buf.drain(..=last_newline);
        }

        // Force-flush output that never contains a newline (binary, progress bars).
        if buf.len() > MAX_LINE_BUFFER {
            let line = std::mem::take(&mut *buf);
            self.writer.emit(std::iter::once(line.as_slice()));
        }
    }

    /// Writes any remaining buffered content (an incomplete final line).
    pub fn flush(&self) {
        let mut buf = self.buf.lock().unwrap();
        if !buf.is_empty() {
            let line = std::mem::take(&mut *buf);
            self.writer.emit(std::iter::once(line.as_slice()));
        }
    }
}

/// Right-pads `s` with spaces to at least `width` columns.
fn pad(s: &str, width: usize) -> String {
    format!("{s:<width$}")
}

/// Joins lines into one buffer, each prefixed and newline-terminated, so a
/// whole batch reaches the sink as a single write.
fn join_lines<'a>(prefix: &str, lines: impl Iterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut buf = Vec::new();
    for line in lines {
        buf.extend_from_slice(prefix.as_bytes());
        buf.extend_from_slice(line);
        buf.push(b'\n');
    }
    buf
}
