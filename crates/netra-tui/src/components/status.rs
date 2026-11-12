//! The bottom status line.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::truncate;

/// Renders the status text, clipped to the available width.
pub fn render(frame: &mut Frame<'_>, area: Rect, text: &str) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text = truncate(text, usize::from(area.width));
    let line = Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)));
    frame.render_widget(Paragraph::new(line), area);
}
