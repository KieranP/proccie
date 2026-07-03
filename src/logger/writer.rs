//! A writer for one tag: buffers raw output into whole lines, emits leveled
//! messages, copies to an optional log file, routes to an ANSI stream or store.

use std::borrow::Cow;
use std::fs::OpenOptions;
use std::io::{LineWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Arc, Mutex};

use anstyle::{Color, Style};

use super::{LogLevel, LogStore, Source};
use crate::sync::MutexExt;
use crate::theme::Theme;

/// Maximum bytes buffered before a forced flush, bounding memory for sources
/// that emit large output without newlines.
pub const MAX_LINE_BUFFER: usize = 1024 * 1024; // 1 MiB

/// Permission mode for log files this writer creates.
const LOG_FILE_PERMS: u32 = 0o600;

/// The shared destination every writer routes to: an ANSI stream
/// (piped/headless), or a structured store (each writer pushes to its own).
pub(super) enum Output {
    /// ANSI-prefixed lines to a writer (often stdout).
    Stream(Mutex<Box<dyn Write + Send>>),
    /// Structured push: each writer pushes [`LogLine`](super::LogLine)s into
    /// its own store for a UI to render.
    Store,
}

/// State a [`Logger`](super::Logger) and every [`TaggedWriter`] it mints share:
/// the destination, the prefix pad width, and the level threshold.
pub(super) struct Core {
    pub(super) out: Output,
    pub(super) pad_width: usize,
    pub(super) level: LogLevel,
    pub(super) theme: Theme,
}

/// Precomputed prefixes/styling for one line category (raw output or a severity
/// level), built once per writer so neither `write` nor `log` allocates per line.
struct Prefixes {
    /// Colored stream prefix: `<tag…> | ` (raw) or `<tag [LEVEL]…> | ` (leveled).
    prefix: String,
    /// Plain, uncolored log-file prefix.
    plain_prefix: String,
    /// Content opener/reset wrapped around each line: empty for raw output (so a
    /// program's own ANSI passes through), the severity style for leveled lines.
    open: String,
    reset: String,
    /// The prefix color used when storing structurally.
    color: Color,
}

impl Prefixes {
    /// Prefix set for raw output: the tag colored, content left unstyled so a
    /// program's own ANSI passes through.
    fn raw(tag: &str, color: Color, width: usize) -> Prefixes {
        Prefixes::build(tag, Style::new().fg_color(Some(color)), color, width, false)
    }

    /// Prefix set for a leveled line: the severity style wraps both the
    /// `tag [LEVEL]` prefix and the content.
    fn leveled(tag: &str, level: LogLevel, width: usize, theme: Theme) -> Prefixes {
        Prefixes::build(
            &leveled_tag(tag, level),
            level.style(theme),
            level.color(theme),
            width,
            true,
        )
    }

    /// Builds a prefix set: `label` padded to `width`, styled for the stream and
    /// stored plain. `style_content` wraps each line's content in the style too.
    fn build(
        label: &str,
        style: Style,
        color: Color,
        width: usize,
        style_content: bool,
    ) -> Prefixes {
        let padded = pad(label, width);
        let open = style.render().to_string();
        let reset = style.render_reset().to_string();
        Prefixes {
            prefix: format!("{open}{padded}{reset} | "),
            plain_prefix: line_prefix(label, width),
            open: if style_content { open } else { String::new() },
            reset: if style_content { reset } else { String::new() },
            color,
        }
    }
}

/// A log sink for one tag: the shared [`Core`], prefix sets, the optional log
/// file, its store, and a line buffer. Created via [`Logger::tagged_writer`](super::Logger).
pub struct TaggedWriter {
    core: Arc<Core>,
    tag: Arc<str>,
    store: Arc<LogStore>,
    base: Prefixes,
    leveled: [Prefixes; 4],
    log_file: Option<Mutex<Box<dyn Write + Send>>>,
    buf: Mutex<Vec<u8>>,
}

impl TaggedWriter {
    /// Builds a writer for `tag` sharing `core`, opening `log_file` (a path) for
    /// appending if given, and precomputing its colored/plain raw prefixes.
    pub(super) fn new(
        core: Arc<Core>,
        tag: &str,
        color: Color,
        store: Arc<LogStore>,
        log_file: Option<&str>,
    ) -> std::io::Result<Arc<TaggedWriter>> {
        let file = match log_file {
            Some(path) => Some(Mutex::new(open_log_file(path)?)),
            None => None,
        };
        Ok(TaggedWriter::with_file(core, tag, color, store, file))
    }

    /// Builds a writer with an already-resolved (or absent) log file. Infallible,
    /// so a file-less sink (the built-in system writer) needs no error handling.
    pub(super) fn with_file(
        core: Arc<Core>,
        tag: &str,
        color: Color,
        store: Arc<LogStore>,
        log_file: Option<Mutex<Box<dyn Write + Send>>>,
    ) -> Arc<TaggedWriter> {
        // Shared once, then cloned per line as a cheap refcount bump.
        let tag: Arc<str> = Arc::from(tag);
        let base = Prefixes::raw(&tag, color, core.pad_width);
        // One set per level; `emit` indexes by `level as usize`, matching
        // `LogLevel::ALL`'s order.
        let leveled =
            LogLevel::ALL.map(|lvl| Prefixes::leveled(&tag, lvl, core.pad_width, core.theme));

        Arc::new(TaggedWriter {
            core,
            base,
            leveled,
            tag,
            store,
            log_file,
            buf: Mutex::new(Vec::new()),
        })
    }

    /// This writer's structured store (the reader side for a UI).
    pub fn store(&self) -> &Arc<LogStore> {
        &self.store
    }

    /// Whether the destination is a structured store rather than a stream.
    pub fn is_store_mode(&self) -> bool {
        matches!(self.core.out, Output::Store)
    }

    /// Appends raw `data`, emitting each completed line with the tag prefix.
    /// Output is held until a newline, or force-flushed past [`MAX_LINE_BUFFER`].
    pub fn write(&self, data: &[u8]) {
        let mut buf = self.buf.lock_recover();
        // Held bytes are always newline-free (every path below drains through
        // the last newline), so only the new chunk needs scanning.
        let held = buf.len();
        buf.extend_from_slice(data);

        if let Some(last_newline) = data.iter().rposition(|&b| b == b'\n').map(|i| held + i) {
            self.emit(None, buf[..last_newline].split(|&b| b == b'\n'));
            buf.drain(..=last_newline);
        }

        // Force-flush output that never contains a newline (binary, progress bars).
        if buf.len() > MAX_LINE_BUFFER {
            let line = std::mem::take(&mut *buf);
            self.emit(None, std::iter::once(line.as_slice()));
        }
    }

    /// Writes any remaining buffered content (an incomplete final line).
    pub fn flush(&self) {
        let mut buf = self.buf.lock_recover();
        if !buf.is_empty() {
            let line = std::mem::take(&mut *buf);
            self.emit(None, std::iter::once(line.as_slice()));
        }
    }

    /// Emits an always-shown note line (never level-gated): a structured line
    /// colored `color` in store mode, a raw prefixed line when streaming.
    pub fn note(&self, color: Color, msg: impl AsRef<str>) {
        let msg = msg.as_ref();
        match &self.core.out {
            Output::Store => {
                for line in msg.split('\n') {
                    self.store.push(
                        Source {
                            tag: self.tag.clone(),
                            level: None,
                        },
                        color,
                        line.to_owned(),
                    );
                }
            }
            Output::Stream(_) => self.emit(None, msg.split('\n').map(str::as_bytes)),
        }
    }

    /// Emits a leveled message tagged `tag [LEVEL]`, styled by severity, only
    /// when `level` meets the configured threshold.
    pub fn log(&self, level: LogLevel, msg: impl AsRef<str>) {
        if level < self.core.level {
            return;
        }
        let msg = msg.as_ref();
        let lines = msg.trim_end_matches('\n').split('\n').map(str::as_bytes);
        self.emit(Some(level), lines);
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

    /// Routes a batch of lines (`level` selects the prefix set; `None` = raw) to
    /// the log file, if any, and then to the configured stream or store.
    fn emit<'a>(&self, level: Option<LogLevel>, lines: impl Iterator<Item = &'a [u8]> + Clone) {
        let p = match level {
            None => &self.base,
            Some(lvl) => &self.leveled[lvl as usize],
        };

        self.emit_to_file(p, level, lines.clone());
        match &self.core.out {
            Output::Stream(out) => Self::emit_to_stream(out, p, lines),
            Output::Store => self.emit_to_store(p, level, lines),
        }
    }

    /// Copies a plain, ANSI-stripped batch to the per-process log file, if one is
    /// configured (the prefix is uncolored; the program's own escapes are stripped).
    fn emit_to_file<'a>(
        &self,
        p: &Prefixes,
        level: Option<LogLevel>,
        lines: impl Iterator<Item = &'a [u8]>,
    ) {
        let Some(log_file) = &self.log_file else {
            return;
        };
        let mut buf = Vec::new();
        for line in lines {
            buf.extend_from_slice(p.plain_prefix.as_bytes());
            buf.extend_from_slice(&strip_ansi(level, line));
            buf.push(b'\n');
        }
        let _ = log_file.lock_recover().write_all(&buf);
    }

    /// Writes a colored, prefixed batch to the ANSI stream in a single write.
    fn emit_to_stream<'a>(
        out: &Mutex<Box<dyn Write + Send>>,
        p: &Prefixes,
        lines: impl Iterator<Item = &'a [u8]>,
    ) {
        let _ = out
            .lock_recover()
            .write_all(&join_lines(&p.prefix, &p.open, &p.reset, lines));
    }

    /// Pushes each line (ANSI stripped) into the store, tagged for the UI to render.
    fn emit_to_store<'a>(
        &self,
        p: &Prefixes,
        level: Option<LogLevel>,
        lines: impl Iterator<Item = &'a [u8]>,
    ) {
        for line in lines {
            let text = String::from_utf8_lossy(&strip_ansi(level, line)).into_owned();
            self.store.push(
                Source {
                    tag: self.tag.clone(),
                    level,
                },
                p.color,
                text,
            );
        }
    }
}

/// The displayed tag for a leveled message, e.g. `system [WARN]`.
pub fn leveled_tag(tag: &str, level: LogLevel) -> String {
    format!("{tag} [{}]", level.label())
}

/// Right-pads `s` with spaces to at least `width` columns.
pub(super) fn pad(s: &str, width: usize) -> String {
    format!("{s:<width$}")
}

/// The plain, uncolored prefix for a log line: `label` padded to `width`, then
/// the ` | ` divider. Shared by stream output and a combined UI view.
pub fn line_prefix(label: &str, width: usize) -> String {
    format!("{} | ", pad(label, width))
}

/// Opens `path` for appending (creating it), wrapped in a `LineWriter` so each
/// prefixed line reaches the file as a single write.
fn open_log_file(path: &str) -> std::io::Result<Box<dyn Write + Send>> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(LOG_FILE_PERMS)
        // Don't follow a symlink target; a pre-planted link mustn't redirect appends.
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)?;
    Ok(Box::new(LineWriter::new(file)))
}

/// Strips ANSI escapes from a program's raw (`None`-level) output; leveled lines
/// are proccie's own and carry none, so they pass through without a scan or copy.
fn strip_ansi(level: Option<LogLevel>, line: &[u8]) -> Cow<'_, [u8]> {
    match level {
        None if line.contains(&0x1b) => Cow::Owned(anstream::adapter::strip_bytes(line).into_vec()),
        _ => Cow::Borrowed(line),
    }
}

/// Joins lines into one newline-terminated buffer — each prefixed, content
/// wrapped in `open`/`reset` — so a whole batch reaches the sink in one write.
fn join_lines<'a>(
    prefix: &str,
    open: &str,
    reset: &str,
    lines: impl Iterator<Item = &'a [u8]>,
) -> Vec<u8> {
    let mut buf = Vec::new();
    for line in lines {
        buf.extend_from_slice(prefix.as_bytes());
        buf.extend_from_slice(open.as_bytes());
        buf.extend_from_slice(line);
        buf.extend_from_slice(reset.as_bytes());
        buf.push(b'\n');
    }
    buf
}
