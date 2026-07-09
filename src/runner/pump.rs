//! Copies a child's merged stdout/stderr into its writer and, in output-watch
//! mode, scans that stream for the readiness needle. Used by [`lifecycle`](super::lifecycle).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::logger::TaggedWriter;
use crate::service::Service;

use super::probe::Needle;

/// Idle grace while draining output after a child exits: the pump is abandoned
/// once no further output arrives for this long (a lingering grandchild's open pipe).
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Absolute cap on draining, so a grandchild that keeps *writing* (never idle)
/// can't drain forever and hang the run; the idle grace handles the common case.
const OUTPUT_DRAIN_MAX: Duration = Duration::from_secs(10);

/// Chunks buffered between the async reader and the blocking stream writer; a
/// stalled consumer backs up through this to the pipe rather than growing unbounded.
const PUMP_QUEUE_DEPTH: usize = 256;

/// The output-watch handoff, moved to the pump so its EOF drops the last sender:
/// the (smart-case) needle to find and the signal set once the process's own
/// output contains it.
pub(crate) struct OutputWatch {
    pub(crate) needle: Needle,
    pub(crate) matched: watch::Sender<bool>,
}

/// Copies a child output stream into the service's writer until EOF or a read
/// error, flushing the trailing partial line. `read` counts bytes pumped; when
/// `output_watch` is set, each chunk is also scanned for the readiness needle.
pub(crate) async fn pump<R: AsyncRead + Unpin>(
    mut reader: R,
    writer: Arc<TaggedWriter>,
    system: Arc<TaggedWriter>,
    label: String,
    read: Arc<AtomicU64>,
    output_watch: Option<OutputWatch>,
) {
    let mut buf = [0u8; 8192];
    let mut scanner = output_watch.map(OutputScanner::new);

    if writer.is_store_mode() {
        // Store writes only touch memory, so run them inline on this task.
        while let Some(n) = read_step(&mut reader, &mut buf, &system, &label).await {
            read.fetch_add(n as u64, Ordering::Relaxed);
            if let Some(s) = &mut scanner {
                s.feed(&buf[..n]);
            }
            writer.write(&buf[..n]);
        }
        writer.flush();
        return;
    }

    // Stream writes can block on a stalled consumer, so one blocking task drains a
    // bounded channel; a full channel backs pressure up to the pipe (and the child).
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(PUMP_QUEUE_DEPTH);
    let sink_writer = Arc::clone(&writer);
    let sink = tokio::task::spawn_blocking(move || {
        while let Some(chunk) = rx.blocking_recv() {
            sink_writer.write(&chunk);
        }
        sink_writer.flush();
    });
    while let Some(n) = read_step(&mut reader, &mut buf, &system, &label).await {
        read.fetch_add(n as u64, Ordering::Relaxed);
        // Scan before handing off: readiness must not wait on a stalled sink.
        if let Some(s) = &mut scanner {
            s.feed(&buf[..n]);
        }
        // A closed channel means the sink died; nothing more can be written.
        if tx.send(buf[..n].to_vec()).await.is_err() {
            break;
        }
    }
    // Dropping `tx` closes the channel, ending the sink (which does the final flush).
    drop(tx);
    let _ = sink.await;
}

/// Scans a child's output for a readiness needle (ANSI stripped, carrying a tail
/// across reads), tripping `matched` on the first occurrence, then goes inert.
struct OutputScanner {
    watch: OutputWatch,
    /// Stateful ANSI stripper; holds an escape sequence split across reads.
    strip: anstream::adapter::StripBytes,
    carry: Vec<u8>,
    done: bool,
}

impl OutputScanner {
    fn new(watch: OutputWatch) -> OutputScanner {
        OutputScanner {
            watch,
            strip: anstream::adapter::StripBytes::new(),
            carry: Vec::new(),
            done: false,
        }
    }

    /// Feeds one chunk, signalling readiness the first time the needle appears.
    fn feed(&mut self, chunk: &[u8]) {
        if self.done {
            return;
        }
        // Prepend the carry, then append this chunk's printable (ANSI-stripped) bytes.
        let mut hay = std::mem::take(&mut self.carry);
        for printable in self.strip.strip_next(chunk) {
            hay.extend_from_slice(printable);
        }
        if self.watch.needle.contains(&hay) {
            self.done = true;
            // A dropped receiver means the poller is gone; the match no longer matters.
            let _ = self.watch.matched.send(true);
            return;
        }
        // Retain only enough tail to complete a needle that spans the next read.
        let keep = self.watch.needle.byte_len().saturating_sub(1);
        self.carry = hay.split_off(hay.len().saturating_sub(keep));
    }
}

/// One read into `buf`: returns the chunk length, or `None` on EOF or a (logged)
/// read error. The shared read/EOF/error step for both pump modes.
async fn read_step<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut [u8],
    system: &TaggedWriter,
    label: &str,
) -> Option<usize> {
    match reader.read(buf).await {
        Ok(0) => None,
        Ok(n) => Some(n),
        Err(e) => {
            system.warn(format!("error reading {label}: {e}"));
            None
        }
    }
}

/// Drains the pump after the child exits, abandoning it once output stalls (a
/// lingering grandchild's idle pipe) or the absolute [`OUTPUT_DRAIN_MAX`] cap.
pub(crate) async fn drain_output(out_task: &mut JoinHandle<()>, read: &AtomicU64, svc: &Service) {
    let deadline = tokio::time::Instant::now() + OUTPUT_DRAIN_MAX;
    loop {
        let before = read.load(Ordering::Relaxed);
        // Wait up to the idle grace, but never past the absolute cap.
        let step =
            OUTPUT_DRAIN_GRACE.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        if tokio::time::timeout(step, &mut *out_task).await.is_ok() {
            return;
        }
        // Keep draining while output still flows and the cap allows it.
        if read.load(Ordering::Relaxed) != before && tokio::time::Instant::now() < deadline {
            continue;
        }
        out_task.abort();
        // Store mode flushes inline (abort skips it); the stream sink flushes on close.
        if svc.logger().is_store_mode() {
            svc.logger().flush();
        }
        return;
    }
}
