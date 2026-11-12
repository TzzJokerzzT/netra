//! Fallback message for terminals smaller than the supported layout.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::Paragraph,
};

use crate::app::{MIN_HEIGHT, MIN_WIDTH};

/// Renders a single readable message instead of a broken layout.
pub fn render(frame: &mut Frame<'_>, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let lines = vec![
        Line::from("Terminal too small"),
        Line::from(format!("netra needs at least {MIN_WIDTH}x{MIN_HEIGHT}")),
        Line::from(format!("current: {}x{}", area.width, area.height)),
        Line::from("resize the window or press q to quit"),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(Color::Yellow)),
        area,
    );
}
