//! The tab bar: one colored label per tab, the active one underlined.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Tabs;

use crate::service::Service;
use crate::tui::app::{App, Tab};
use crate::tui::color::{status_color, to_ratatui};

use super::ALL_TITLE;

/// Renders the tab bar into `area`.
pub(super) fn render(frame: &mut Frame, app: &App, area: Rect) {
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
