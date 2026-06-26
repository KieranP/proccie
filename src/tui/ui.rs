//! Pure rendering: the tab bar, the bordered log viewport, and the footer.
//! Takes `&App` and emits widgets; no I/O, so it is unit-testable.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};

use crate::logger::{LogLine, leveled_tag, line_prefix, merge_tail};
use crate::service::{Service, ServiceStatus};

use super::app::{App, Tab};
use super::color::{status_color, system_style, to_ratatui};

/// Color of the trailing unread-output marker.
const UNREAD_COLOR: Color = Color::Yellow;

/// Title for the combined view tab.
const ALL_TITLE: &str = "All";

/// Background fill marking the active tab (a subtle dark-gray pill).
const TAB_ACTIVE_BG: Color = Color::Indexed(238);

/// Color of the thin divider drawn between tabs.
const TAB_DIVIDER: Color = Color::DarkGray;

/// Renders the whole UI into `frame`.
pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // tab bar
        Constraint::Min(0),    // log viewport
        Constraint::Length(1), // footer
    ])
    .split(frame.area());

    render_tabs(frame, app, chunks[0]);
    render_viewport(frame, app, chunks[1]);
    render_footer(frame, app, chunks[2]);
}

/// Renders the tab bar: a status-colored pill per tab, the active one filled.
fn render_tabs(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let titles: Vec<Line> = app
        .tabs()
        .iter()
        .map(|tab| match tab {
            Tab::All => all_tab_label(),
            Tab::Service(i) => service_tab_label(app, &app.services[*i]),
        })
        .collect();

    let tabs = Tabs::new(titles)
        .select(app.active())
        .divider(Span::styled("│", Style::default().fg(TAB_DIVIDER)))
        // A filled pill keeps each tab's own colors, unlike a harsh reverse.
        .highlight_style(
            Style::default()
                .bg(TAB_ACTIVE_BG)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

/// Builds the combined-view tab label: a neutral list glyph plus "All".
fn all_tab_label() -> Line<'static> {
    // Gray (not DarkGray) so the glyph still reads on the active-tab pill.
    Line::from(vec![
        Span::styled("≡", Style::default().fg(Color::Gray)),
        Span::raw(format!(" {ALL_TITLE}")),
    ])
}

/// Builds a service tab label: a status icon (status-colored, with the exit
/// code when noteworthy), the name in its service color, then an unread dot.
fn service_tab_label(app: &App, svc: &Service) -> Line<'static> {
    let status = svc.status();
    let icon_style = Style::default().fg(status_color(status));
    // Borrow the bare glyph (no allocation); only the rare coded case formats.
    let mut spans = match status.code() {
        None => vec![Span::styled(status.icon(), icon_style)],
        Some(code) => vec![Span::styled(format!("{}{code}", status.icon()), icon_style)],
    };
    spans.push(Span::raw(" "));

    // The name in its service color, dimmed once the service is terminal.
    let mut name_style = Style::default().fg(to_ratatui(svc.color()));
    if status.is_terminal() {
        name_style = name_style.add_modifier(Modifier::DIM);
    }
    spans.push(Span::styled(svc.label().to_string(), name_style));

    if app.has_unread(svc) {
        spans.push(Span::styled(" •", Style::default().fg(UNREAD_COLOR)));
    }
    Line::from(spans)
}

/// Renders the bordered log viewport for the active tab.
fn render_viewport(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let inner_height = area.height.saturating_sub(2) as usize; // top + bottom border
    let scroll = app.active_scroll();
    let lines = visible_lines(app, scroll.offset_from_bottom, inner_height);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(tab_title(app, app.active_tab()));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Selects and styles the visible window of the active tab's lines.
fn visible_lines(app: &App, offset: usize, height: usize) -> Vec<Line<'static>> {
    let (is_all, raw) = match app.active_tab() {
        Tab::All => (true, merge_tail(app.all_stores(), offset + height)),
        Tab::Service(i) => (false, app.services[i].log().tail(offset + height)),
    };

    let end = raw.len().saturating_sub(offset);
    let start = end.saturating_sub(height);
    // `raw` is discarded here, so move each line's text into its span.
    raw.into_iter()
        .skip(start)
        .take(end - start)
        .map(|line| {
            if is_all {
                all_line(app, line)
            } else {
                service_line(line)
            }
        })
        .collect()
}

/// Renders one All-view line: the source's colored, padded label prefix, then
/// the text (system lines style the whole line by severity).
fn all_line(app: &App, line: LogLine) -> Line<'static> {
    let label = match line.source.level {
        Some(level) => leveled_tag(&line.source.tag, level),
        None => line.source.tag.to_string(),
    };
    let padded = line_prefix(&label, app.prefix_width);
    let color = to_ratatui(line.color);

    match line.source.level {
        None => Line::from(vec![
            Span::styled(padded, Style::default().fg(color)),
            Span::raw(line.text),
        ]),
        Some(level) => {
            let style = system_style(level, color);
            Line::from(vec![
                Span::styled(padded, style),
                Span::styled(line.text, style),
            ])
        }
    }
}

/// Renders one service-tab line: just the text (the tab label already carries
/// the service color), left plain except for severity styling on leveled lines.
fn service_line(line: LogLine) -> Line<'static> {
    match line.source.level {
        None => Line::from(Span::raw(line.text)),
        Some(level) => {
            let style = system_style(level, to_ratatui(line.color));
            Line::from(Span::styled(line.text, style))
        }
    }
}

/// The plain title for a tab: the combined view, or the service's display label.
fn tab_title(app: &App, tab: Tab) -> String {
    match tab {
        Tab::All => ALL_TITLE.to_string(),
        Tab::Service(i) => app.services[i].label().to_string(),
    }
}

/// Renders the footer: keybinding hints and the run status.
fn render_footer(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
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

/// The keybinding hints, with `c: close` appended on a closeable service tab.
fn footer_hints(app: &App) -> String {
    let mut hints =
        "Tab/⇧Tab: switch   ↑↓/PgUp/PgDn/Home/End: scroll   Ctrl+C: stop/quit   q: quit"
            .to_string();
    if app.active_service().is_some_and(Service::is_closeable) {
        hints.push_str("   c: close");
    }
    hints
}

/// The run status: the exit code once finished, else the per-state service tally.
fn footer_status(app: &App) -> String {
    if let Some(code) = app.finished {
        return format!("exited (code {code}) — Ctrl+C/q to quit");
    }
    if app.is_shutting_down() {
        return "shutting down… (Ctrl+C/q again to force-kill)".to_string();
    }
    if app.is_exiting() {
        return "exiting…".to_string();
    }

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
