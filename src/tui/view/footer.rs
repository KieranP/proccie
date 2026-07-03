//! The footer: the search box while searching, else keybinding hints and the
//! run status.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::logger::query_is_case_sensitive;
use crate::service::{Service, ServiceStatus};
use crate::tui::app::{App, Tab};

/// Renders the footer into `area`.
pub(super) fn render(frame: &mut Frame, app: &App, area: Rect) {
    if let Some((query, cursor)) = app.search_editing() {
        frame.render_widget(Paragraph::new(editing_footer(app, query, cursor)), area);
        return;
    }
    if let Some(query) = app.search_committed() {
        frame.render_widget(Paragraph::new(committed_footer(app, query)), area);
        return;
    }
    let footer = Line::from(vec![
        Span::styled(
            footer_hints(app),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::raw("   |   "),
        Span::raw(footer_status(app)),
    ]);
    frame.render_widget(Paragraph::new(footer), area);
}

/// The editing footer: query + caret, the match count (or a hint), and the keys
/// to keep or clear the filter.
fn editing_footer(app: &App, query: &str, cursor: usize) -> Line<'static> {
    let trailer = if query.is_empty() {
        "   type to filter · Esc: cancel".to_string()
    } else {
        let n = match_count(app, query);
        format!(
            "   {n} match{} · Enter: keep · Esc: clear · Ctrl+A/E: ends",
            plural(n)
        )
    };
    let mut spans = vec![Span::styled(
        "Search: ",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    spans.extend(query_with_caret(query, cursor));
    spans.push(Span::styled(
        trailer,
        Style::default().add_modifier(Modifier::DIM),
    ));
    append_status(&mut spans, app);
    Line::from(spans)
}

/// The committed-filter footer (box shut, filter live): query, match count, and
/// the keys to re-edit or clear it.
fn committed_footer(app: &App, query: &str) -> Line<'static> {
    let n = match_count(app, query);
    let mut spans = vec![
        Span::styled("Filter: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(query.to_string()),
        Span::styled(
            format!("   {n} match{} · s: edit · Esc: clear", plural(n)),
            Style::default().add_modifier(Modifier::DIM),
        ),
    ];
    append_status(&mut spans, app);
    Line::from(spans)
}

/// Renders `query` with a reverse-video caret at `cursor` (a block past the end).
fn query_with_caret(query: &str, cursor: usize) -> Vec<Span<'static>> {
    let caret = Style::default().add_modifier(Modifier::REVERSED);
    let (before, rest) = query.split_at(cursor);
    match rest.chars().next() {
        Some(c) => vec![
            Span::raw(before.to_string()),
            Span::styled(c.to_string(), caret),
            Span::raw(rest[c.len_utf8()..].to_string()),
        ],
        None => vec![Span::raw(before.to_string()), Span::styled(" ", caret)],
    }
}

/// `""`/`"es"` so a count reads as "1 match" / "2 matches".
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "es" }
}

/// The number of lines in the active tab matching `query` (smart case).
fn match_count(app: &App, query: &str) -> usize {
    let case_sensitive = query_is_case_sensitive(query);
    match app.active_tab() {
        Tab::All => app
            .all_stores()
            .map(|s| s.count_matching(query, case_sensitive))
            .sum(),
        Tab::Service(i) => app.services[i].log().count_matching(query, case_sensitive),
    }
}

/// Appends the run status to a search footer when it is noteworthy, so a
/// shutdown, exit, or finish stays visible even with the box open.
fn append_status(spans: &mut Vec<Span<'static>>, app: &App) {
    if let Some(status) = alert_status(app) {
        spans.push(Span::raw("   |   "));
        spans.push(Span::raw(status));
    }
}

/// The keybinding hints, with `c: close` appended on a closeable service tab.
fn footer_hints(app: &App) -> String {
    let mut hints =
        "Tab/⇧Tab: switch   ↑↓/PgUp/PgDn/Home/End: scroll   s: search   Ctrl+C: stop/quit   q: quit"
            .to_string();
    if app.active_service().is_some_and(Service::is_closeable) {
        hints.push_str("   c: close");
    }
    hints
}

/// The run status: an alert (finished/shutting down/exiting) when there is one,
/// else the per-state service tally.
fn footer_status(app: &App) -> String {
    alert_status(app).unwrap_or_else(|| service_tally(app))
}

/// A run status worth surfacing even mid-search; `None` during a normal run,
/// when only the tally would show.
fn alert_status(app: &App) -> Option<String> {
    if let Some(code) = app.finished {
        Some(format!("exited (code {code}) — Ctrl+C/q to quit"))
    } else if app.is_shutting_down() {
        Some("shutting down… (Ctrl+C/q again to force-kill)".to_string())
    } else if app.is_exiting() {
        Some("exiting…".to_string())
    } else {
        None
    }
}

/// The per-state service tally: non-zero states only, in lifecycle order.
fn service_tally(app: &App) -> String {
    // Tally into a stack array indexed by ServiceStatus::ALL (no per-frame heap
    // allocation); each status finds its slot by variant.
    let mut counts = [0usize; ServiceStatus::ALL.len()];
    for svc in app.services.iter() {
        let status = svc.status();
        if let Some(i) = ServiceStatus::ALL
            .iter()
            .position(|s| std::mem::discriminant(s) == std::mem::discriminant(&status))
        {
            counts[i] += 1;
        }
    }

    // Show only the non-zero states, in lifecycle order; labels come from noun().
    ServiceStatus::ALL
        .iter()
        .zip(counts)
        .filter(|&(_, n)| n > 0)
        .map(|(status, n)| format!("{n} {}", status.noun()))
        .collect::<Vec<_>>()
        .join(", ")
}
