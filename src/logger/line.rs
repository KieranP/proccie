//! The structured log primitives stored by [`LogStore`](super::LogStore):
//! UI-agnostic, carrying `anstyle::Color` (mapped to a UI color at render time).

use std::sync::Arc;

use anstyle::Color;

use super::level::LogLevel;

/// Where a stored log line came from: a (shared) tag, plus a severity `level`
/// for leveled (system) messages — `None` for plain tagged output.
#[derive(Debug, Clone)]
pub struct Source {
    pub tag: Arc<str>,
    pub level: Option<LogLevel>,
}

/// One stored, structured log line. `color` is the display color of its
/// prefix; tag-only views render `text`, a combined view adds the prefix.
#[derive(Debug, Clone)]
pub struct LogLine {
    /// Globally increasing arrival stamp, so stores merge in true order.
    pub seq: u64,
    pub source: Source,
    pub color: Color,
    pub text: String,
}
