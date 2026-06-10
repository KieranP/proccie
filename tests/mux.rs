//! Tests for the colored, line-buffered log multiplexer.

mod common;

use proccie::mux::{LogLevel, MAX_LINE_BUFFER, Mux};

use common::{SharedBuf, build_mux};

const ESC: &str = "\x1b[";

#[test]
fn emits_complete_lines_with_prefix() {
    let (mux, out) = build_mux(6, LogLevel::Debug);
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
    let (mux, out) = build_mux(4, LogLevel::Debug);
    let w = mux.prefix_writer("app", None).stream();

    w.write(b"hel");
    w.write(b"lo\n");

    assert_eq!(out.contents().lines().count(), 1);
    assert!(out.contents().contains("hello"));
}

#[test]
fn flush_emits_incomplete_final_line() {
    let (mux, out) = build_mux(4, LogLevel::Debug);
    let w = mux.prefix_writer("app", None).stream();

    w.write(b"no newline");
    assert!(out.is_empty(), "nothing should be written before flush");

    w.flush();
    assert!(out.contents().contains("no newline"));
}

#[test]
fn large_write_without_newline_is_force_flushed() {
    let (mux, out) = build_mux(4, LogLevel::Debug);
    let w = mux.prefix_writer("app", None).stream();

    w.write(&vec![b'x'; MAX_LINE_BUFFER + 100]);
    assert!(!out.is_empty());
}

#[test]
fn log_lines_below_threshold_are_suppressed() {
    // At the warn threshold, debug and info are dropped; warn and error show.
    let (mux, out) = build_mux(6, LogLevel::Warn);
    mux.debug("debug line");
    mux.info("info line");
    assert!(out.is_empty(), "{}", out.contents());

    mux.warn("warn line");
    mux.error("error line");
    let output = out.contents();
    assert!(
        output.contains("warn line") && output.contains("error line"),
        "{output}"
    );
}

#[test]
fn log_prefix_carries_the_level() {
    let (mux, out) = build_mux(6, LogLevel::Debug);
    mux.info("hello world");
    let output = out.contents();
    assert!(output.contains("system [INFO]"), "{output}");
    assert!(output.contains("hello world"), "{output}");
}

#[test]
fn log_level_parses_case_insensitively_and_orders() {
    assert_eq!("DEBUG".parse(), Ok(LogLevel::Debug));
    assert_eq!("Info".parse(), Ok(LogLevel::Info));
    assert_eq!("warning".parse(), Ok(LogLevel::Warn));
    assert_eq!("error".parse(), Ok(LogLevel::Error));
    assert!("loud".parse::<LogLevel>().is_err());

    assert!(LogLevel::Debug < LogLevel::Info);
    assert!(LogLevel::Info < LogLevel::Warn);
    assert!(LogLevel::Warn < LogLevel::Error);
}

#[test]
fn prefix_width_measures_chars_not_bytes() {
    // Short names align to the widest system tag, `system [DEBUG]` (14 chars).
    assert_eq!(Mux::prefix_width(["café", "data"]), 14);
    // A name longer than the system tag wins, measured in chars not bytes:
    // "café-server-xyz" is 15 chars but 16 bytes.
    assert_eq!(Mux::prefix_width(["café-server-xyz"]), 15);
}

#[test]
fn log_file_copy_is_plain_text() {
    let (mux, console) = build_mux(6, LogLevel::Debug);
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
    let (mux, console) = build_mux(4, LogLevel::Debug);
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
