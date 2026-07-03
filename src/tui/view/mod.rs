//! Pure rendering split by region ([`tabs`], [`viewport`], [`footer`]); also
//! holds the layout, scroll clamp, and line building the regions share.

mod footer;
mod tabs;
mod viewport;

use std::rc::Rc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::logger::{
    LogLine, leveled_tag, line_prefix, merge_tail, merge_tail_matching, query_is_case_sensitive,
};

use super::app::{App, Tab};
use super::color::{system_style, to_ratatui};

/// Title for the combined view tab.
const ALL_TITLE: &str = "All";

/// Renders the whole UI into `frame`.
pub fn render(frame: &mut Frame, app: &App) {
    let chunks = layout(frame.area());
    tabs::render(frame, app, chunks[0]);
    viewport::render(frame, app, chunks[2]);
    footer::render(frame, app, chunks[3]);
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
    if offset == 0 {
        return offset;
    }
    // Filtering: the match set is bounded and usually small, so just measure it.
    if app.filter_query().is_some() {
        let rows = wrapped_rows(tail_lines(app, usize::MAX), width);
        return offset.min(rows.saturating_sub(height));
    }
    // Cheap exit: enough whole lines that the offset can't overshoot.
    let len = app.content_len();
    if offset.saturating_add(height) <= len {
        return offset;
    }
    // Near the top, measure the wrapped height and pin the offset below it.
    let rows = wrapped_rows(tail_lines(app, len), width);
    offset.min(rows.saturating_sub(height))
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

/// The last `depth` logical lines of the active tab, styled and search-highlighted.
fn tail_lines(app: &App, depth: usize) -> Vec<Line<'static>> {
    let is_all = matches!(app.active_tab(), Tab::All);
    let query = app.filter_query();
    // `raw` is discarded here, so move each line's text into its span.
    raw_tail(app, depth)
        .into_iter()
        .map(|line| {
            if is_all {
                all_line(app, line, query)
            } else {
                service_line(line, query)
            }
        })
        .collect()
}

/// The active tab's last `depth` raw lines. With a search filter active, the
/// store clones only the last `depth` matching lines, not the whole buffer.
fn raw_tail(app: &App, depth: usize) -> Vec<LogLine> {
    let Some(query) = app.filter_query() else {
        return match app.active_tab() {
            Tab::All => merge_tail(app.all_stores(), depth),
            Tab::Service(i) => app.services[i].log().tail(depth),
        };
    };
    let case_sensitive = query_is_case_sensitive(query);
    match app.active_tab() {
        Tab::All => merge_tail_matching(app.all_stores(), depth, query, case_sensitive),
        Tab::Service(i) => app.services[i]
            .log()
            .tail_matching(depth, query, case_sensitive),
    }
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
fn all_line(app: &App, line: LogLine, query: Option<&str>) -> Line<'static> {
    let label = match line.source.level {
        Some(level) => leveled_tag(&line.source.tag, level),
        None => line.source.tag.to_string(),
    };
    let padded = line_prefix(&label, app.prefix_width);
    let color = to_ratatui(line.color);

    let (prefix_style, text_style) = match line.source.level {
        None => (Style::default().fg(color), Style::default()),
        Some(level) => (system_style(level, color), system_style(level, color)),
    };
    let mut spans = vec![Span::styled(padded, prefix_style)];
    spans.extend(highlight(line.text, text_style, query));
    Line::from(spans)
}

/// Renders one service-tab line: just the text (the tab label already carries
/// the service color), left plain except for severity styling on leveled lines.
fn service_line(line: LogLine, query: Option<&str>) -> Line<'static> {
    let style = match line.source.level {
        None => Style::default(),
        Some(level) => system_style(level, to_ratatui(line.color)),
    };
    Line::from(highlight(line.text, style, query))
}

/// Splits `text` into spans, reverse-highlighting matches of `query` (smart
/// case, like the filter); the rest keeps `base`. No query is one plain span.
fn highlight(text: String, base: Style, query: Option<&str>) -> Vec<Span<'static>> {
    let Some(query) = query.filter(|q| !q.is_empty()) else {
        return vec![Span::styled(text, base)];
    };
    let mark = base.add_modifier(Modifier::REVERSED);

    // Case-insensitive folds both to ASCII lower — same byte offsets, valid slices.
    let lower_hay;
    let lower_needle;
    let (hay, needle): (&str, &str) = if query_is_case_sensitive(query) {
        (text.as_str(), query)
    } else {
        lower_hay = text.to_ascii_lowercase();
        lower_needle = query.to_ascii_lowercase();
        (lower_hay.as_str(), lower_needle.as_str())
    };

    let mut spans = Vec::new();
    let mut last = 0;
    while let Some(rel) = hay[last..].find(needle) {
        let start = last + rel;
        let end = start + needle.len();
        if start > last {
            spans.push(Span::styled(text[last..start].to_string(), base));
        }
        spans.push(Span::styled(text[start..end].to_string(), mark));
        last = end;
    }
    if last < text.len() {
        spans.push(Span::styled(text[last..].to_string(), base));
    }
    // No match (predicate and highlighter can differ on non-ASCII): one plain span.
    if spans.is_empty() {
        spans.push(Span::styled(text, base));
    }
    spans
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
    fn search_filters_the_view_to_matching_lines() {
        let (mut app, _svc, _dir) = app_with_wrapping_lines();
        app.search_for_test("end-03");
        let screen = draw(&mut app);

        assert!(
            screen.contains("end-03"),
            "matching line missing from filtered view:\n{screen}"
        );
        assert!(
            !screen.contains("end-06"),
            "non-matching line should be filtered out:\n{screen}"
        );
        // The footer shows the query and the live match count.
        assert!(
            screen.contains("Search: end-03") && screen.contains("1 match"),
            "search footer missing query/count:\n{screen}"
        );
    }

    #[test]
    fn smart_case_makes_uppercase_queries_case_sensitive() {
        let (mut app, _svc, _dir) = app_with_wrapping_lines();

        // The lines are all lowercase; an uppercase query matches nothing.
        app.search_for_test("END");
        let screen = draw(&mut app);
        assert!(
            screen.contains("0 matches"),
            "uppercase query should be case-sensitive:\n{screen}"
        );
        assert!(
            !screen.contains("end-03"),
            "no lines should survive a case-sensitive miss:\n{screen}"
        );

        // A lowercase query folds case and matches every line's "end-0x".
        app.search_for_test("end");
        let screen = draw(&mut app);
        assert!(
            screen.contains("6 matches"),
            "lowercase query should match case-insensitively:\n{screen}"
        );
    }

    #[test]
    fn search_footer_keeps_the_run_status_and_hints_cursor_keys() {
        let (mut app, _svc, _dir) = app_with_wrapping_lines();
        app.on_runner_finished(Some(0)); // the run has exited
        app.search_for_test("end");

        // Render wide so the appended footer segments aren't truncated.
        let mut terminal = Terminal::new(TestBackend::new(120, HEIGHT)).expect("terminal");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let screen = terminal.backend().to_string();

        assert!(
            screen.contains("exited (code 0)"),
            "run status hidden while searching:\n{screen}"
        );
        assert!(
            screen.contains("Ctrl+A/E"),
            "cursor-jump keys not hinted:\n{screen}"
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
