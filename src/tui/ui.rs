//! Pure rendering: the tab bar, the bordered log viewport, and the footer.
//! Takes `&App` and emits widgets; no I/O, so it is unit-testable.

use std::rc::Rc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs, Wrap};

use crate::logger::{LogLine, leveled_tag, line_prefix, merge_tail};
use crate::service::{Service, ServiceStatus};

use super::app::{App, Tab};
use super::color::{status_color, system_style, to_ratatui};

/// Title for the combined view tab.
const ALL_TITLE: &str = "All";

/// Renders the whole UI into `frame`.
pub fn render(frame: &mut Frame, app: &App) {
    let chunks = layout(frame.area());
    render_tabs(frame, app, chunks[0]);
    render_viewport(frame, app, chunks[2]);
    render_footer(frame, app, chunks[3]);
}

/// The single source of truth for the vertical layout: tab bar, spacer,
/// viewport, footer. `split` is cached, so callers share the result.
fn layout(area: Rect) -> Rc<[Rect]> {
    Layout::vertical([
        Constraint::Length(1), // tab bar
        Constraint::Length(1), // spacer between the tabs and the content
        Constraint::Min(0),    // log viewport
        Constraint::Length(1), // footer
    ])
    .split(area)
}

/// The viewport's inner size (width, height in rows) at `area`, sans borders —
/// the geometry the event loop clamps and pages against.
pub(super) fn viewport_size(area: Rect) -> (u16, usize) {
    let inner = layout(area)[2];
    (
        inner.width.saturating_sub(2),           // left + right border
        inner.height.saturating_sub(2) as usize, // top + bottom border
    )
}

/// Clamps a scroll offset (rows from the bottom) to the reachable maximum for
/// the active tab at `width`/`height`, measuring only when near the top.
pub(super) fn clamp_scroll(app: &App, width: u16, height: usize) -> usize {
    let offset = app.active_scroll().offset_from_bottom;
    let len = app.content_len();
    // Cheap exits: following, or enough whole lines that the offset can't overshoot.
    if offset == 0 || offset.saturating_add(height) <= len {
        return offset;
    }
    // Near the top, measure the wrapped height and pin the offset below it.
    let rows = wrapped_rows(tail_lines(app, len), width);
    offset.min(rows.saturating_sub(height))
}

/// Renders the tab bar: one colored label per tab, the active one underlined.
fn render_tabs(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let titles: Vec<Line> = app
        .tabs()
        .iter()
        .map(|tab| match tab {
            Tab::All => all_tab_label(app),
            Tab::Service(i) => service_tab_label(app, &app.services[*i]),
        })
        .collect();

    let tabs = Tabs::new(titles)
        .select(app.active())
        .divider(Span::styled(
            "│",
            Style::default().fg(to_ratatui(app.theme.subtle())),
        ))
        // One color for the whole active tab (no DIM) keeps its underline uniform.
        .highlight_style(
            Style::default()
                .fg(active_tab_color(app))
                .add_modifier(Modifier::UNDERLINED | Modifier::BOLD)
                .remove_modifier(Modifier::DIM),
        );
    frame.render_widget(tabs, area);
}

/// Builds the combined-view tab label: a neutral list glyph plus "All".
fn all_tab_label(app: &App) -> Line<'static> {
    // Neutral glyph; the word keeps the terminal default to contrast on any bg.
    Line::from(vec![
        Span::styled("≡", Style::default().fg(to_ratatui(app.theme.subtle()))),
        Span::raw(format!(" {ALL_TITLE}")),
    ])
}

/// Builds a service tab label: a status icon (status-colored, with the exit
/// code when noteworthy), the name in its service color, then an unread dot.
fn service_tab_label(app: &App, svc: &Service) -> Line<'static> {
    let status = svc.status();
    let icon_style = Style::default().fg(status_color(status, app.theme));
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
        spans.push(Span::styled(
            " •",
            Style::default().fg(to_ratatui(app.theme.warning())),
        ));
    }
    Line::from(spans)
}

/// The active tab's representative color, tinting its whole label and underline:
/// the service's own color, or the neutral accent for the combined view.
fn active_tab_color(app: &App) -> Color {
    match app.active_tab() {
        Tab::All => to_ratatui(app.theme.subtle()),
        Tab::Service(i) => to_ratatui(app.services[i].color()),
    }
}

/// Renders the bordered log viewport for the active tab. Long lines wrap; the
/// window shows the `inner_height` rows ending `offset` up from the newest.
fn render_viewport(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let inner_height = area.height.saturating_sub(2) as usize; // top + bottom border
    let inner_width = area.width.saturating_sub(2); // left + right border
    let offset = app.active_scroll().offset_from_bottom;

    // Keep only the newest lines covering the window plus the rows hidden below.
    let mut lines = tail_lines(app, offset.saturating_add(inner_height));
    let needed = inner_height.saturating_add(offset);
    let mut rows = 0;
    let mut start = lines.len();
    while start > 0 && rows < needed {
        start -= 1;
        rows += wrapped_rows(vec![lines[start].clone()], inner_width);
    }
    lines.drain(..start); // drop lines entirely above the window

    let block = Block::default()
        .borders(Borders::ALL)
        .title(tab_title(app, app.active_tab()));
    // Skip the overshoot above the window; the rows below it are clipped.
    let scroll_y = rows.saturating_sub(needed).min(u16::MAX as usize) as u16;
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y, 0));
    frame.render_widget(paragraph, area);
}

/// The last `depth` logical lines of the active tab, styled for display.
fn tail_lines(app: &App, depth: usize) -> Vec<Line<'static>> {
    let (is_all, raw) = match app.active_tab() {
        Tab::All => (true, merge_tail(app.all_stores(), depth)),
        Tab::Service(i) => (false, app.services[i].log().tail(depth)),
    };
    // `raw` is discarded here, so move each line's text into its span.
    raw.into_iter()
        .map(|line| {
            if is_all {
                all_line(app, line)
            } else {
                service_line(line)
            }
        })
        .collect()
}

/// Rows `lines` occupy wrapped to `width` (no borders). `trim: false` wraps each
/// logical line alone, so it serves both the tail measure and the per-line walk.
fn wrapped_rows(lines: Vec<Line<'static>>, width: u16) -> usize {
    if lines.is_empty() {
        return 0;
    }
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .line_count(width)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::config::Config;
    use crate::logger::{Destination, LogLevel, Logger, Source};
    use crate::service::Service;
    use crate::theme::Theme;

    const WIDTH: u16 = 30;
    const HEIGHT: u16 = 12;
    // Matches the event loop: viewport height/width minus the surrounding chrome.
    const INNER_HEIGHT: usize = HEIGHT as usize - 5;
    const INNER_WIDTH: u16 = WIDTH - 2;

    /// Builds an app focused on one service tab holding six lines, each far wider
    /// than the 28-column inner width so every line wraps to two rows.
    fn app_with_wrapping_lines() -> (App, Arc<[Service]>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("Procfile.toml");
        std::fs::write(&path, "[svc]\ncommand = \"true\"\n").expect("write config");
        let config = Config::load(&path).expect("config loads");
        let logger = Logger::new(
            Destination::Store,
            config.names(),
            LogLevel::Debug,
            Theme::Dark,
        );
        let adjacency = config.adjacency();
        let services =
            Service::build_all(&config, &adjacency, &logger, Theme::Dark).expect("build services");
        let system = Arc::clone(logger.system().store());
        let mut app = App::new(
            Arc::clone(&services),
            system,
            logger.pad_width(),
            Theme::Dark,
        );

        for i in 1..=6 {
            services[0].log().push(
                Source {
                    tag: services[0].label().into(),
                    level: None,
                },
                anstyle::AnsiColor::Cyan.into(),
                format!("start-{i:02} aaaa bbbb cccc dddd eeee ffff end-{i:02}"),
            );
        }
        app.next_tab(); // focus the service tab (index 1)
        app.set_viewport(INNER_HEIGHT);
        (app, services, dir)
    }

    /// Draws one frame like the event loop does: clamp the scroll to the measured
    /// height, then render, and return the screen as text.
    fn draw(app: &mut App) -> String {
        let offset = clamp_scroll(app, INNER_WIDTH, INNER_HEIGHT);
        app.set_scroll_offset(offset);
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("terminal");
        terminal.draw(|f| render(f, app)).expect("draw");
        terminal.backend().to_string()
    }

    #[test]
    fn long_lines_wrap_and_keep_the_newest_visible() {
        let (mut app, _svc, _dir) = app_with_wrapping_lines();
        let screen = draw(&mut app);

        // `end-06` past the wrap column proves wrapping, and the tail is visible.
        assert!(
            screen.contains("end-06"),
            "newest wrapped tail missing:\n{screen}"
        );
        let end05 = screen.find("end-05").expect("end-05 on screen");
        let end06 = screen.find("end-06").expect("end-06 on screen");
        assert!(end06 > end05, "newest line not below older one:\n{screen}");
    }

    #[test]
    fn mid_scroll_windows_correctly_hiding_newer_and_older_rows() {
        let (mut app, _svc, _dir) = app_with_wrapping_lines();
        app.scroll_up(2); // two rows up from the tail
        let screen = draw(&mut app);

        // Middle lines show; the newest (end-06) is clipped below, oldest above.
        assert!(
            screen.contains("end-05"),
            "mid window missing line 5:\n{screen}"
        );
        assert!(
            screen.contains("end-03"),
            "mid window missing line 3:\n{screen}"
        );
        assert!(
            !screen.contains("end-06"),
            "newest should be below the fold:\n{screen}"
        );
        assert!(
            !screen.contains("start-01"),
            "oldest should be above:\n{screen}"
        );
    }

    #[test]
    fn scroll_to_top_reveals_the_oldest_wrapped_rows() {
        let (mut app, _svc, _dir) = app_with_wrapping_lines();
        app.scroll_home();
        let screen = draw(&mut app);

        // 12 rows (6 lines × 2) into a 7-row viewport → max offset 5.
        assert_eq!(app.active_scroll().offset_from_bottom, 12 - INNER_HEIGHT);
        assert!(
            screen.contains("start-01"),
            "oldest line not reachable at the top:\n{screen}"
        );
        assert!(
            !screen.contains("end-06"),
            "newest line should be scrolled off at the top:\n{screen}"
        );
    }
}
