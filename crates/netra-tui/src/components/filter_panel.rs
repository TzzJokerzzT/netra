//! The filter panel overlay.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use super::centered;
use crate::app::AppState;

/// Renders the current filter and its editing hints.
pub fn render(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let width = area.width.min(64);
    let height = area.height.min(13);
    let popup = centered(area, width, height);
    if popup.width == 0 || popup.height == 0 {
        return;
    }
    frame.render_widget(Clear, popup);
    let filter = &state.filter;
    let mut lines = vec![Line::from(vec![
        Span::styled("text: ", label_style()),
        Span::raw(filter.text.clone().unwrap_or_else(|| "(none)".to_owned())),
    ])];
    if let Some(host) = &filter.host {
        lines.push(field("host", host.clone()));
    }
    if !filter.methods.is_empty() {
        lines.push(field(
            "methods",
            filter
                .methods
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ));
    }
    if !filter.status.is_empty() {
        lines.push(field("status", filter.status.to_text()));
    }
    if !filter.media_kinds.is_empty() {
        lines.push(field(
            "media",
            filter
                .media_kinds
                .iter()
                .map(|kind| kind.label())
                .collect::<Vec<_>>()
                .join(","),
        ));
    }
    if !filter.sources.is_empty() {
        lines.push(field(
            "sources",
            filter
                .sources
                .iter()
                .map(|source| source.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ));
    }
    lines.push(field(
        "only failed",
        if filter.only_failed { "yes" } else { "no" }.to_owned(),
    ));
    lines.push(Line::from(Span::styled(
        format!(
            "{} row(s) hidden by the filter",
            state
                .rows
                .len()
                .saturating_sub(state.rows.iter().filter(|row| filter.matches(row)).count())
        ),
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "/ edits text · F toggles only failed · Esc closes",
        Style::default().fg(Color::DarkGray),
    )));
    let block = Block::bordered()
        .title("Filters")
        .border_style(Style::default().fg(Color::Yellow));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Style of a filter field label.
fn label_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

/// One `label: value` filter line.
fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), label_style()),
        Span::raw(value),
    ])
}
