//! Colored, multiplexed log output for child processes: each line is prefixed
//! with a color-coded name and serialized through a mutex so lines never interleave.

use std::io::{LineWriter, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anstyle::{AnsiColor, Style};

/// Maximum bytes buffered per writer before a forced flush, bounding memory
/// for processes that emit large output without newlines.
pub const MAX_LINE_BUFFER: usize = 1024 * 1024; // 1 MiB

/// Prefix for proccie's own log lines, alongside the process prefixes.
const SYSTEM_PREFIX: &str = "system";

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
    debug: bool,
}

impl Mux {
    /// Width that fits every process name prefix plus the system prefix.
    pub fn prefix_width<'a>(names: impl IntoIterator<Item = &'a str>) -> usize {
        names
            .into_iter()
            .chain(std::iter::once(SYSTEM_PREFIX))
            // `format!` pads by char count, so measure in chars, not bytes.
            .map(|name| name.chars().count())
            .max()
            .unwrap_or(0)
    }

    /// Creates a `Mux` writing to `out`, padding name prefixes to `pad_width`.
    /// System log lines are emitted only when `debug` is set.
    pub fn new(out: impl Write + Send + 'static, pad_width: usize, debug: bool) -> Arc<Mux> {
        Arc::new(Mux {
            // Every emitted batch ends in '\n', so LineWriter passes it
            // through to `out` as a single write.
            out: Mutex::new(Box::new(LineWriter::new(out))),
            color_idx: AtomicUsize::new(0),
            pad_width,
            debug,
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

    /// Writes a system-level message with a dimmed "system" prefix. The
    /// message is only written when debug mode is enabled.
    pub fn system_log(&self, msg: impl AsRef<str>) {
        if !self.debug {
            return;
        }

        let dim = Style::new().dimmed();
        let padded = pad(SYSTEM_PREFIX, self.pad_width);

        let mut out = self.out.lock().unwrap();
        for line in msg.as_ref().trim_end_matches('\n').split('\n') {
            let _ = writeln!(
                out,
                "{open}{padded}{reset} | {open}{line}{reset}",
                open = dim.render(),
                reset = dim.render_reset(),
            );
        }
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
