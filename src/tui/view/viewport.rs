//! The bordered log viewport: windows the active tab's lines to the visible
//! rows and lays them into the scrolled, bordered widget.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::app::{App, Tab};

use super::{ALL_TITLE, tail_lines, wrapped_rows};

/// Renders the log viewport into `area`. Long lines wrap; the window shows the
/// inner-height rows ending `offset` up from the newest.
pub(super) fn render(frame: &mut Frame, app: &App, area: Rect) {
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
    let scroll_y = u16::try_from(rows.saturating_sub(needed)).unwrap_or(u16::MAX);
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_y, 0));
    frame.render_widget(paragraph, area);
}

/// The plain title for a tab: the combined view, or the service's display label.
fn tab_title(app: &App, tab: Tab) -> String {
    match tab {
        Tab::All => ALL_TITLE.to_string(),
        Tab::Service(i) => app.services[i].label().to_string(),
    }
}
