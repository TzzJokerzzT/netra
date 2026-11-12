//! The confirmation dialog used before destructive or replay actions.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
};

use super::centered;
use crate::app::ConfirmDialog;

/// Renders the dialog and its two answers.
pub fn render(frame: &mut Frame<'_>, area: Rect, dialog: &ConfirmDialog) {
    let width = area.width.min(58);
    let height = area.height.min(9);
    let popup = centered(area, width, height);
    if popup.width == 0 || popup.height == 0 {
        return;
    }
    frame.render_widget(Clear, popup);
    let lines = vec![
        Line::from(Span::styled(
            dialog.message.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                " y ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" confirm    "),
            Span::styled(
                " n ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" cancel"),
        ]),
        Line::from(Span::styled(
            "Enter confirms · Esc cancels",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let block = Block::bordered()
        .title("Confirm")
        .border_style(Style::default().fg(Color::Yellow));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}
