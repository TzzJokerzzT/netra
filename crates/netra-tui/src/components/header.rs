//! The single-line session header.

use netra_core::domain::DurationMs;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::SessionHeader;

/// Renders the session header into one line.
pub fn render(frame: &mut Frame<'_>, area: Rect, header: &SessionHeader) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let name = if header.name.is_empty() {
        "unnamed session".to_owned()
    } else {
        header.name.clone()
    };
    let id = header
        .session_id
        .map_or_else(|| "--------".to_owned(), |id| id.short());
    let elapsed = elapsed_label(header.elapsed);
    let spans = vec![
        Span::styled(
            " netra ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(name, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" [{id}]"), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("  {}", header.status),
            Style::default().fg(status_color(&header.status)),
        ),
        Span::styled(
            format!("  src:{}", header.sources),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(format!("  {} req", header.total)),
        Span::styled(
            format!("  {} err", header.errors),
            Style::default().fg(Color::Red),
        ),
        Span::styled(format!("  {elapsed}"), Style::default().fg(Color::DarkGray)),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Colour for a short session status label.
#[must_use]
pub fn status_color(status: &str) -> Color {
    match status.trim().to_ascii_lowercase().as_str() {
        "capturing" | "running" => Color::Green,
        "paused" => Color::Yellow,
        "stopped" | "finished" => Color::DarkGray,
        _ => Color::Gray,
    }
}

/// Elapsed time label with an explicit `elapsed` prefix.
fn elapsed_label(elapsed: DurationMs) -> String {
    format!("elapsed {}", elapsed)
}
