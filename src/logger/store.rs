//! In-memory, capped, structured log storage for one tag, shared between the
//! writer that pushes lines and a reader (a UI) that snapshots them.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use anstyle::Color;
use tokio::sync::Notify;

use super::line::{LogLine, Source};
use crate::sync::MutexExt;

/// Per-store line cap; the oldest lines are evicted past it.
pub const MAX_LINES: usize = 10_000;

/// One tag's log buffer. Stores share a `clock` (globally ordered `seq` stamps)
/// and a `redraw` notifier (the UI loop wakes on any push or status change).
pub struct LogStore {
    clock: Arc<AtomicU64>,
    redraw: Arc<Notify>,
    /// Total lines ever pushed; drives the unread mark and is never decremented
    /// on eviction. Atomic so the UI reads it without taking the lines lock.
    total: AtomicU64,
    /// Buffered lines behind one lock: the writer and the reader touch it concurrently.
    lines: Mutex<VecDeque<LogLine>>,
}

impl LogStore {
    /// Creates a store sharing `clock` (seq source) and `redraw` (UI wake-up).
    pub fn new(clock: Arc<AtomicU64>, redraw: Arc<Notify>) -> Arc<LogStore> {
        Arc::new(LogStore {
            clock,
            redraw,
            total: AtomicU64::new(0),
            lines: Mutex::new(VecDeque::new()),
        })
    }

    /// Appends a line, stamping it with the next `seq` and waking the UI.
    pub fn push(&self, source: Source, color: Color, text: String) {
        let seq = self.clock.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(1, Ordering::Relaxed);
        {
            let mut lines = self.lock();
            lines.push_back(LogLine {
                seq,
                source,
                color,
                text,
            });
            if lines.len() > MAX_LINES {
                lines.pop_front();
            }
        }
        self.redraw.notify_one();
    }

    /// Returns the shared redraw notifier, pinged on every push.
    pub fn redraw(&self) -> Arc<Notify> {
        Arc::clone(&self.redraw)
    }

    /// Wakes the UI without pushing a line (e.g. after a status change tracked
    /// outside the store).
    pub fn wake(&self) {
        self.redraw.notify_one();
    }

    /// Returns the total number of lines ever pushed (eviction-stable).
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Snapshots the last `depth` buffered lines (oldest first).
    pub fn tail(&self, depth: usize) -> Vec<LogLine> {
        let lines = self.lock();
        let start = lines.len().saturating_sub(depth);
        lines.range(start..).cloned().collect()
    }

    /// The `seq`s of the last `depth` buffered lines — cheap, clones no text.
    pub fn tail_seqs(&self, depth: usize) -> Vec<u64> {
        let lines = self.lock();
        let start = lines.len().saturating_sub(depth);
        lines.range(start..).map(|line| line.seq).collect()
    }

    /// Clones the last `depth` buffered lines whose `seq` is at least `min_seq`.
    pub fn tail_since(&self, depth: usize, min_seq: u64) -> Vec<LogLine> {
        let lines = self.lock();
        let start = lines.len().saturating_sub(depth);
        lines
            .range(start..)
            .filter(|line| line.seq >= min_seq)
            .cloned()
            .collect()
    }

    /// Number of currently buffered (non-evicted) lines.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the buffer currently holds no lines.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Locks the lines, recovering from poisoning so one panic can't wedge the UI.
    fn lock(&self) -> MutexGuard<'_, VecDeque<LogLine>> {
        self.lines.lock_recover()
    }
}

/// Merges the `depth` most-recent lines (by `seq`) across `stores`, finding the
/// cutoff from cheap `seq`s first so only surviving lines are cloned per store.
pub fn merge_tail<'a>(
    stores: impl IntoIterator<Item = &'a Arc<LogStore>>,
    depth: usize,
) -> Vec<LogLine> {
    let stores: Vec<&Arc<LogStore>> = stores.into_iter().collect();

    // Partition so the `depth` largest pooled seqs sit at the end; their min is the (exact) cutoff.
    let mut seqs: Vec<u64> = stores.iter().flat_map(|s| s.tail_seqs(depth)).collect();
    let cutoff = if seqs.len() > depth {
        let cut = seqs.len() - depth;
        seqs.select_nth_unstable(cut);
        seqs[cut]
    } else {
        0
    };

    let mut merged: Vec<LogLine> = stores
        .iter()
        .flat_map(|s| s.tail_since(depth, cutoff))
        .collect();
    merged.sort_by_key(|line| line.seq);
    merged
}
