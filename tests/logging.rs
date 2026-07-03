//! Tests for the structured log store: capping/eviction and cross-store merge.

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anstyle::AnsiColor;
use tokio::sync::Notify;

use proccie::logger::{
    LogLevel, LogStore, MAX_LINES, Source, merge_tail, merge_tail_matching,
    query_is_case_sensitive, text_matches,
};

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
fn text_matches_is_smart_case() {
    // A lowercase query is case-insensitive.
    assert!(!query_is_case_sensitive("err"));
    assert!(text_matches("ERROR here", "err", false));
    assert!(text_matches("an error", "err", false));

    // An uppercase letter in the query makes it case-sensitive.
    assert!(query_is_case_sensitive("ERR"));
    assert!(text_matches("ERROR here", "ERR", true));
    assert!(!text_matches("an error", "ERR", true));

    // An empty query matches everything, so a fresh box shows the whole log.
    assert!(text_matches("anything", "", false));
}

#[test]
fn count_matching_honors_case_sensitivity() {
    let s = store();
    s.push(tagged("s"), color(), "Error: boom".into());
    s.push(tagged("s"), color(), "error: fizz".into());
    s.push(tagged("s"), color(), "all clear".into());

    assert_eq!(s.count_matching("error", false), 2); // case-insensitive
    assert_eq!(s.count_matching("Error", true), 1); // case-sensitive
}

#[test]
fn tail_matching_returns_the_last_matches_oldest_first() {
    let s = store();
    for i in 0..10 {
        let kind = if i % 2 == 0 { "hit" } else { "miss" };
        s.push(tagged("s"), color(), format!("line {i} {kind}"));
    }
    // Even lines are hits (0,2,4,6,8); the last two are 6 and 8, oldest first.
    let texts: Vec<String> = s
        .tail_matching(2, "hit", false)
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert_eq!(texts, ["line 6 hit", "line 8 hit"]);
}

#[test]
fn merge_tail_matching_takes_last_matches_across_stores_by_seq() {
    let clock = Arc::new(AtomicU64::new(0));
    let redraw = Arc::new(Notify::new());
    let a = LogStore::new(Arc::clone(&clock), Arc::clone(&redraw));
    let b = LogStore::new(clock, redraw);

    // Shared clock; matches for "x" land at seq 0, 1, 3, 4.
    a.push(tagged("a"), color(), "a x".into()); // 0
    b.push(tagged("b"), color(), "b x".into()); // 1
    a.push(tagged("a"), color(), "a no".into()); // 2 (miss)
    b.push(tagged("b"), color(), "b x".into()); // 3
    a.push(tagged("a"), color(), "a x".into()); // 4

    // The last two matches by seq are 3 and 4, ordered by seq.
    let merged = merge_tail_matching(&[a, b], 2, "x", false);
    let texts: Vec<&str> = merged.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["b x", "a x"]);
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
