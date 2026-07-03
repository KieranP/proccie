//! Tests for the structured log store: capping/eviction and cross-store merge.

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anstyle::AnsiColor;
use tokio::sync::Notify;

use proccie::logger::{LogLevel, LogStore, MAX_LINES, Source, merge_tail};

fn color() -> anstyle::Color {
    AnsiColor::Cyan.into()
}

fn tagged(tag: &str) -> Source {
    Source {
        tag: tag.into(),
        level: None,
    }
}

fn store() -> Arc<LogStore> {
    LogStore::new(Arc::new(AtomicU64::new(0)), Arc::new(Notify::new()))
}

#[test]
fn buffer_caps_and_front_evicts_keeping_total() {
    let s = store();
    let extra = 50;
    for i in 0..(MAX_LINES + extra) {
        s.push(tagged("svc"), color(), format!("line {i}"));
    }

    assert_eq!(s.len(), MAX_LINES);
    // `total` is eviction-stable, driving the unread mark.
    assert_eq!(usize::try_from(s.total()).unwrap(), MAX_LINES + extra);

    let lines = s.tail(usize::MAX);
    assert_eq!(lines.first().unwrap().text, format!("line {extra}"));
    assert_eq!(
        lines.last().unwrap().text,
        format!("line {}", MAX_LINES + extra - 1)
    );
}

#[test]
fn merge_tail_orders_by_seq_across_stores() {
    let clock = Arc::new(AtomicU64::new(0));
    let redraw = Arc::new(Notify::new());
    let a = LogStore::new(Arc::clone(&clock), Arc::clone(&redraw));
    let b = LogStore::new(clock, redraw);

    // Interleave pushes; the shared clock stamps a globally increasing seq.
    a.push(tagged("a"), color(), "a1".into());
    b.push(tagged("b"), color(), "b1".into());
    a.push(tagged("a"), color(), "a2".into());
    b.push(
        Source {
            tag: "sys".into(),
            level: Some(LogLevel::Info),
        },
        color(),
        "sys".into(),
    );

    let merged = merge_tail(&[a, b], 10);
    let texts: Vec<&str> = merged.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["a1", "b1", "a2", "sys"]);
    assert!(merged.windows(2).all(|w| w[0].seq < w[1].seq));
}

#[test]
fn merge_tail_keeps_only_the_last_depth() {
    let s = store();
    for i in 0..20 {
        s.push(tagged("s"), color(), format!("{i}"));
    }
    let merged = merge_tail(&[s], 5);
    let texts: Vec<&str> = merged.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["15", "16", "17", "18", "19"]);
}
