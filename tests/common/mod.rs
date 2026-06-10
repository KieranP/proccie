//! Shared helpers for integration tests.

#![allow(dead_code)]

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use proccie::mux::Mux;
use tempfile::TempDir;

/// An in-memory writer that can be cloned to share one buffer between a
/// [`Mux`](proccie::mux::Mux) and the test that inspects its output.
#[derive(Clone, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    pub fn new() -> SharedBuf {
        SharedBuf::default()
    }

    /// Returns the buffer contents as a lossy UTF-8 string.
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Builds a `Mux` that captures its output in the returned [`SharedBuf`].
pub fn build_mux(pad_width: usize, debug: bool) -> (Arc<Mux>, SharedBuf) {
    let out = SharedBuf::new();
    let mux = Mux::new(out.clone(), pad_width, debug);
    (mux, out)
}

/// Writes `content` to a `Procfile.toml` inside a fresh temp directory and
/// returns the directory (keep it alive) and the file path.
pub fn write_config(content: &str) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("Procfile.toml");
    std::fs::write(&path, content).expect("write config");
    (dir, path)
}

/// Polls `buf` until `needle` appears or `timeout` elapses.
pub async fn wait_for_output(buf: &SharedBuf, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if buf.contents().contains(needle) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    buf.contents().contains(needle)
}
