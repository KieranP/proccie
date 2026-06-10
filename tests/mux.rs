//! Tests for the colored, line-buffered log multiplexer.

mod common;

use proccie::mux::{MAX_LINE_BUFFER, Mux};

use common::{SharedBuf, build_mux};

const ESC: &str = "\x1b[";

#[test]
fn emits_complete_lines_with_prefix() {
    let (mux, out) = build_mux(6, true);
    let w = mux.prefix_writer("web", None).stream();

    w.write(b"hello\nworld\n");

    let output = out.contents();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("hello") && lines[0].contains("web"));
    assert!(lines[1].contains("world"));
}

#[test]
fn buffers_partial_lines_until_newline() {
    let (mux, out) = build_mux(4, true);
    let w = mux.prefix_writer("app", None).stream();

    w.write(b"hel");
    w.write(b"lo\n");

    assert_eq!(out.contents().lines().count(), 1);
    assert!(out.contents().contains("hello"));
}

#[test]
fn flush_emits_incomplete_final_line() {
    let (mux, out) = build_mux(4, true);
    let w = mux.prefix_writer("app", None).stream();

    w.write(b"no newline");
    assert!(out.is_empty(), "nothing should be written before flush");

    w.flush();
    assert!(out.contents().contains("no newline"));
}

#[test]
fn large_write_without_newline_is_force_flushed() {
    let (mux, out) = build_mux(4, true);
    let w = mux.prefix_writer("app", None).stream();

    w.write(&vec![b'x'; MAX_LINE_BUFFER + 100]);
    assert!(!out.is_empty());
}

#[test]
fn system_log_is_gated_on_debug() {
    let (mux, out) = build_mux(6, false);
    mux.system_log("should not appear");
    assert!(out.is_empty());

    let (mux, out) = build_mux(6, true);
    mux.system_log("hello world");
    assert!(out.contents().contains("system"));
    assert!(out.contents().contains("hello world"));
}

#[test]
fn prefix_width_measures_chars_not_bytes() {
    // A 4-char, 5-byte name must align with a 4-char ASCII name.
    assert_eq!(Mux::prefix_width(["café", "data"]), 6); // "system" wins
    assert_eq!(Mux::prefix_width(["café-server"]), 11);
}

#[test]
fn log_file_copy_is_plain_text() {
    let (mux, console) = build_mux(6, true);
    let log_file = SharedBuf::new();
    let w = mux
        .prefix_writer("web", Some(Box::new(log_file.clone())))
        .stream();

    w.write(b"hello\n");

    // Console output is colored.
    assert!(console.contents().contains(ESC));
    assert!(console.contents().contains("hello"));
    // The log file copy is plain and prefixed.
    let log = log_file.contents();
    assert!(!log.contains(ESC));
    assert!(log.contains("hello") && log.contains("web"));
}

#[test]
fn each_process_writes_to_its_own_log_file() {
    let (mux, console) = build_mux(4, true);
    let log_a = SharedBuf::new();
    let log_b = SharedBuf::new();

    let a = mux
        .prefix_writer("aaa", Some(Box::new(log_a.clone())))
        .stream();
    let b = mux
        .prefix_writer("bbb", Some(Box::new(log_b.clone())))
        .stream();
    a.write(b"from a\n");
    b.write(b"from b\n");

    assert!(log_a.contents().contains("from a") && !log_a.contents().contains("from b"));
    assert!(log_b.contents().contains("from b") && !log_b.contents().contains("from a"));
    assert!(console.contents().contains("from a") && console.contents().contains("from b"));
}
