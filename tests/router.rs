//! Tests for the tagged, line-buffered log writer and the logger's destination.

mod common;

use anstyle::AnsiColor;
use tempfile::tempdir;

use proccie::logger::{LogLevel, MAX_LINE_BUFFER};

use common::build_logger;

const ESC: &str = "\x1b[";

/// A fixed color for tagged-writer tests (stream mode ignores the store).
fn color() -> anstyle::Color {
    AnsiColor::Cyan.into()
}

#[test]
fn emits_complete_lines_with_prefix() {
    let (logger, out) = build_logger(&["web"], LogLevel::Debug);
    let w = logger.tagged_writer("web", color(), None).unwrap();

    w.write(b"hello\nworld\n");

    let output = out.contents();
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("hello") && lines[0].contains("web"));
    assert!(lines[1].contains("world"));
}

#[test]
fn buffers_partial_lines_until_newline() {
    let (logger, out) = build_logger(&["app"], LogLevel::Debug);
    let w = logger.tagged_writer("app", color(), None).unwrap();

    w.write(b"hel");
    w.write(b"lo\n");

    assert_eq!(out.contents().lines().count(), 1);
    assert!(out.contents().contains("hello"));
}

#[test]
fn flush_emits_incomplete_final_line() {
    let (logger, out) = build_logger(&["app"], LogLevel::Debug);
    let w = logger.tagged_writer("app", color(), None).unwrap();

    w.write(b"no newline");
    assert!(out.is_empty(), "nothing should be written before flush");

    w.flush();
    assert!(out.contents().contains("no newline"));
}

#[test]
fn large_write_without_newline_is_force_flushed() {
    let (logger, out) = build_logger(&["app"], LogLevel::Debug);
    let w = logger.tagged_writer("app", color(), None).unwrap();

    w.write(&vec![b'x'; MAX_LINE_BUFFER + 100]);
    assert!(!out.is_empty());
}

#[test]
fn leveled_lines_below_threshold_are_suppressed() {
    // At the warn threshold, debug and info are dropped; warn and error show.
    let (logger, out) = build_logger(&[], LogLevel::Warn);
    logger.system().debug("debug line");
    logger.system().info("info line");
    assert!(out.is_empty(), "{}", out.contents());

    logger.system().warn("warn line");
    logger.system().error("error line");
    let output = out.contents();
    assert!(
        output.contains("warn line") && output.contains("error line"),
        "{output}"
    );
}

#[test]
fn leveled_prefix_carries_the_tag_and_level() {
    let (logger, out) = build_logger(&[], LogLevel::Debug);
    logger.system().info("hello world");
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
fn pad_width_measures_chars_not_bytes() {
    // Short labels align to the widest leveled prefix, `system [DEBUG]` (14 chars).
    let (logger, _out) = build_logger(&["café", "data"], LogLevel::Debug);
    assert_eq!(logger.pad_width(), 14);
    // A label longer than the leveled prefix wins, measured in chars not bytes:
    // "café-server-xyz" is 15 chars but 16 bytes.
    let (logger, _out) = build_logger(&["café-server-xyz"], LogLevel::Debug);
    assert_eq!(logger.pad_width(), 15);
}

#[test]
fn log_file_copy_is_plain_text() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("web.log");
    let (logger, console) = build_logger(&["web"], LogLevel::Debug);
    let w = logger.tagged_writer("web", color(), path.to_str()).unwrap();

    w.write(b"hello\n");

    // Console output is colored.
    assert!(console.contents().contains(ESC));
    assert!(console.contents().contains("hello"));
    // The log file copy is plain and prefixed.
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(!log.contains(ESC));
    assert!(log.contains("hello") && log.contains("web"));
}

#[test]
fn each_tag_writes_to_its_own_log_file() {
    let dir = tempdir().unwrap();
    let path_a = dir.path().join("aaa.log");
    let path_b = dir.path().join("bbb.log");
    let (logger, console) = build_logger(&["aaa", "bbb"], LogLevel::Debug);

    let a = logger
        .tagged_writer("aaa", color(), path_a.to_str())
        .unwrap();
    let b = logger
        .tagged_writer("bbb", color(), path_b.to_str())
        .unwrap();
    a.write(b"from a\n");
    b.write(b"from b\n");

    let log_a = std::fs::read_to_string(&path_a).unwrap();
    let log_b = std::fs::read_to_string(&path_b).unwrap();
    assert!(log_a.contains("from a") && !log_a.contains("from b"));
    assert!(log_b.contains("from b") && !log_b.contains("from a"));
    assert!(console.contents().contains("from a") && console.contents().contains("from b"));
}
