//! A self-contained, UI-agnostic logging subsystem: structured primitives,
//! capped per-tag stores, and the `Logger` writer factory.

mod level;
mod line;
mod store;
mod writer;

pub use level::{Emphasis, LogLevel};
pub use line::{LogLine, Source};
pub use store::{LogStore, MAX_LINES, merge_tail};
pub use writer::{MAX_LINE_BUFFER, TaggedWriter, leveled_tag, line_prefix};

use std::io::{LineWriter, Write};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use anstyle::Color;
use tokio::sync::Notify;

use crate::theme::Theme;
use writer::{Core, Output};

/// Tag for the logger's own (system) messages.
const SYSTEM_TAG: &str = "system";

/// Where a logger sends its lines: an ANSI stream (piped/headless), or a
/// structured store the UI reads back (the logger mints one store per tag).
pub enum Destination {
    /// Auto-styled ANSI lines to stdout; styling is stripped when it isn't a TTY.
    Stream,
    /// ANSI lines to a caller-supplied writer, raw (e.g. test output capture).
    Writer(Box<dyn Write + Send>),
    /// Structured push: each writer pushes into the store the logger mints for it.
    Store,
}

/// Owns the shared [`Core`] (destination, pad width, level) and hands out
/// [`TaggedWriter`]s that route through it, plus a built-in `system` writer.
pub struct Logger {
    core: Arc<Core>,
    /// Shared by every store this logger mints: globally ordered `seq` stamps
    /// and the single UI redraw notifier.
    clock: Arc<AtomicU64>,
    redraw: Arc<Notify>,
    system: Arc<TaggedWriter>,
}

impl Logger {
    /// Creates a logger sending to `dest` (plus a built-in `system` writer), the prefix
    /// pad width sized to fit `labels`, and leveled lines below `level` suppressed.
    pub fn new<'a>(
        dest: Destination,
        labels: impl IntoIterator<Item = &'a str>,
        level: LogLevel,
        theme: Theme,
    ) -> Arc<Logger> {
        // Coordination primitives shared by every store this logger mints.
        let clock = Arc::new(AtomicU64::new(0));
        let redraw = Arc::new(Notify::new());

        // The destination and formatting that all writers route through.
        let out = match dest {
            Destination::Stream => {
                stream_output(Box::new(anstream::AutoStream::auto(std::io::stdout())))
            }
            Destination::Writer(w) => stream_output(w),
            Destination::Store => Output::Store,
        };
        let core = Arc::new(Core {
            out,
            pad_width: prefix_width(labels),
            level,
            theme,
        });

        // The built-in writer for the logger's own messages (no log file).
        let system = TaggedWriter::with_file(
            Arc::clone(&core),
            SYSTEM_TAG,
            theme.subtle(),
            LogStore::new(Arc::clone(&clock), Arc::clone(&redraw)),
            None,
        );

        Arc::new(Logger {
            core,
            clock,
            redraw,
            system,
        })
    }

    /// The writer for the logger's own (system) leveled messages.
    pub fn system(&self) -> &Arc<TaggedWriter> {
        &self.system
    }

    /// The shared prefix pad width every writer aligns to.
    pub fn pad_width(&self) -> usize {
        self.core.pad_width
    }

    /// Creates a writer for `tag`: raw lines prefixed in `color`, copied to the
    /// `log_file` path (opened here) if given, over a freshly minted store.
    pub fn tagged_writer(
        &self,
        tag: &str,
        color: Color,
        log_file: Option<&str>,
    ) -> std::io::Result<Arc<TaggedWriter>> {
        TaggedWriter::new(
            Arc::clone(&self.core),
            tag,
            color,
            LogStore::new(Arc::clone(&self.clock), Arc::clone(&self.redraw)),
            log_file,
        )
    }
}

/// Wraps a writer as a stream destination: a `LineWriter` so each
/// '\n'-terminated batch reaches the writer as a single write.
fn stream_output(w: Box<dyn Write + Send>) -> Output {
    Output::Stream(Mutex::new(Box::new(LineWriter::new(w))))
}

/// Pad width that fits every `labels` entry plus the system writer's widest
/// leveled prefix (`system [DEBUG]`), so all prefixes align.
fn prefix_width<'a>(labels: impl IntoIterator<Item = &'a str>) -> usize {
    let label_widths = labels.into_iter().map(|t| t.chars().count());
    let system_widths = LogLevel::ALL
        .iter()
        .map(|lvl| leveled_tag(SYSTEM_TAG, *lvl).chars().count());
    label_widths.chain(system_widths).max().unwrap_or(0)
}
