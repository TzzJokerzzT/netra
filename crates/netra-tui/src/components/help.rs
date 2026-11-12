//! The help overlay.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use super::centered;

/// Key binding rows shown in the overlay.
const BINDINGS: &[(&str, &str)] = &[
    ("j / k · ↓ / ↑", "move the selection"),
    ("g / G", "first / last request"),
    ("Enter", "focus the inspector"),
    ("Tab / Shift-Tab", "next / previous pane or tab"),
    ("1 … 7", "select an inspector tab"),
    ("/", "search the visible text"),
    ("f / F", "filter panel / only failed"),
    ("c", "clear filters, selection and tab"),
    ("s · p · e", "save · pause · export HAR"),
    ("r · d", "replay · delete (both confirmed)"),
    ("? · q", "help · quit"),
];

/// Renders the help overlay in the middle of the screen.
pub fn render(frame: &mut Frame<'_>, area: Rect) {
    let width = area.width.min(62);
    let height = area.height.min(18);
    let popup = centered(area, width, height);
    if popup.width == 0 || popup.height == 0 {
        return;
    }
    frame.render_widget(Clear, popup);
    let mut lines = vec![Line::from(Span::styled(
        "netra captures requests; nothing here leaves the session.",
        Style::default().fg(Color::DarkGray),
    ))];
    for (keys, description) in BINDINGS {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<16}", keys),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw((*description).to_owned()),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "Esc closes the overlay.",
        Style::default().fg(Color::DarkGray),
    )));
    let block = Block::bordered()
        .title("Help")
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}
