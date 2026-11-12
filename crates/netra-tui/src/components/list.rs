//! The request list: one line per exchange with adaptive columns.

use netra_core::domain::{ExchangeState, ExchangeSummary, HttpMethod, StatusCode, human_bytes};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::fit;

/// Width reserved for the method column.
const METHOD_WIDTH: usize = 6;
/// Width reserved for the status column.
const STATUS_WIDTH: usize = 4;

/// Column widths derived from the available space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Columns {
    method: usize,
    url: usize,
    status: usize,
    size: usize,
    duration: usize,
    media: usize,
    source: usize,
}

impl Columns {
    /// Drops columns in priority order until the row fits.
    fn for_width(width: usize) -> Self {
        let mut columns = Self::default();
        if width < 24 {
            columns.url = width;
            return columns;
        }
        columns.method = METHOD_WIDTH.min(width / 4).max(1);
        columns.status = STATUS_WIDTH;
        let mut used = columns.method + 1 + columns.status + 1;
        if width >= used + 8 + 12 {
            columns.size = 7;
            used += 8;
        }
        if width >= used + 8 + 12 {
            columns.duration = 7;
            used += 8;
        }
        if width >= used + 6 + 10 {
            columns.media = 5;
            used += 6;
        }
        if width >= used + 6 + 8 {
            columns.source = 5;
            used += 6;
        }
        columns.url = width.saturating_sub(used);
        columns
    }
}

/// Renders the visible rows, keeping `selection` highlighted.
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    rows: &[&ExchangeSummary],
    selection: usize,
    scroll: usize,
    focused: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let columns = Columns::for_width(usize::from(area.width));
    let visible_height = usize::from(area.height.saturating_sub(1));
    let mut lines = Vec::with_capacity(visible_height.saturating_add(1));
    lines.push(header_line(&columns));
    for (index, row) in rows.iter().enumerate().skip(scroll).take(visible_height) {
        lines.push(row_line(row, &columns, index == selection));
    }
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "no requests match the current filter",
            Style::default().fg(Color::DarkGray),
        )));
    }
    let paragraph = if focused {
        Paragraph::new(lines)
    } else {
        Paragraph::new(lines).style(Style::default().fg(Color::Gray))
    };
    frame.render_widget(paragraph, area);
}

/// Column header line.
fn header_line(columns: &Columns) -> Line<'static> {
    let style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled(fit("METHOD", columns.method), style)];
    let mut push = |span: Span<'static>| spans.push(span);
    push(Span::raw(" "));
    push(Span::styled(fit("URL", columns.url), style));
    push(Span::raw(" "));
    push(Span::styled(fit("CODE", columns.status), style));
    if columns.size > 0 {
        push(Span::raw(" "));
        push(Span::styled(fit("SIZE", columns.size), style));
    }
    if columns.duration > 0 {
        push(Span::raw(" "));
        push(Span::styled(fit("TIME", columns.duration), style));
    }
    if columns.media > 0 {
        push(Span::raw(" "));
        push(Span::styled(fit("KIND", columns.media), style));
    }
    if columns.source > 0 {
        push(Span::raw(" "));
        push(Span::styled(fit("SRC", columns.source), style));
    }
    Line::from(spans)
}

/// One request row.
fn row_line(row: &ExchangeSummary, columns: &Columns, selected: bool) -> Line<'static> {
    let mut spans = vec![Span::styled(
        fit(row.method.as_str(), columns.method),
        method_style(&row.method),
    )];
    spans.push(Span::raw(" "));
    spans.extend(url_spans(row, columns.url));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        fit(&status_text(row.status, row.state), columns.status),
        Style::default().fg(status_color(row.status, row.state)),
    ));
    if columns.size > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::raw(fit(&human_bytes(row.size_bytes), columns.size)));
    }
    if columns.duration > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::raw(fit(
            &row.duration
                .map(|duration| duration.to_string())
                .unwrap_or_default(),
            columns.duration,
        )));
    }
    if columns.media > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            fit(row.media_kind.label(), columns.media),
            Style::default().fg(Color::Magenta),
        ));
    }
    if columns.source > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            fit(row.source.label(), columns.source),
            Style::default().fg(Color::Blue),
        ));
    }
    let line = Line::from(spans);
    if selected {
        line.style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        line
    }
}

/// URL cell: dimmed host plus path and query.
fn url_spans(row: &ExchangeSummary, width: usize) -> Vec<Span<'static>> {
    let path = row.url.display_path();
    if let Some(host) = row.host.as_deref() {
        let host_width = host.chars().count();
        if host_width > 0 && host_width + 6 <= width {
            return vec![
                Span::styled(host.to_owned(), Style::default().fg(Color::DarkGray)),
                Span::raw(fit(&path, width - host_width)),
            ];
        }
    }
    vec![Span::raw(fit(&path, width))]
}

/// Status cell text (`---` while pending, `ERR` for a transport failure).
fn status_text(status: Option<StatusCode>, state: ExchangeState) -> String {
    match status {
        Some(status) => status.to_string(),
        None => match state {
            ExchangeState::Failed => "ERR".to_owned(),
            ExchangeState::Aborted => "ABT".to_owned(),
            _ => "---".to_owned(),
        },
    }
}

/// Colour of the status cell: `2xx` green, `3xx` cyan, `4xx` yellow,
/// `5xx` red and pending grey.
#[must_use]
pub fn status_color(status: Option<StatusCode>, state: ExchangeState) -> Color {
    match status {
        Some(code) if code.is_success() => Color::Green,
        Some(code) if code.is_redirect() => Color::Cyan,
        Some(code) if code.is_client_error() => Color::Yellow,
        Some(code) if code.is_server_error() => Color::Red,
        Some(_) => Color::Gray,
        None => match state {
            ExchangeState::Failed | ExchangeState::Aborted => Color::Red,
            _ => Color::DarkGray,
        },
    }
}

/// Colour of the method cell; safe methods stay readable, mutations stand out.
fn method_style(method: &HttpMethod) -> Style {
    let color = if method.is_safe() {
        Color::Green
    } else if method.is_idempotent() {
        Color::Cyan
    } else {
        Color::Yellow
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: u16) -> Option<StatusCode> {
        StatusCode::try_new(code).ok()
    }

    #[test]
    fn status_classes_get_distinct_colours() {
        assert_eq!(
            status_color(status(200), ExchangeState::Completed),
            Color::Green
        );
        assert_eq!(
            status_color(status(302), ExchangeState::Completed),
            Color::Cyan
        );
        assert_eq!(
            status_color(status(404), ExchangeState::Completed),
            Color::Yellow
        );
        assert_eq!(
            status_color(status(503), ExchangeState::Completed),
            Color::Red
        );
        assert_eq!(status_color(None, ExchangeState::Pending), Color::DarkGray);
        assert_eq!(status_color(None, ExchangeState::Failed), Color::Red);
    }

    #[test]
    fn columns_fit_every_width() {
        for width in 0..=200usize {
            let columns = Columns::for_width(width);
            let used = columns.method
                + columns.url
                + columns.status
                + columns.size
                + columns.duration
                + columns.media
                + columns.source;
            assert!(used <= width.max(1), "width {width} overflowed by {used}");
        }
    }

    #[test]
    fn wide_lists_show_every_column() {
        let columns = Columns::for_width(100);
        assert!(columns.method > 0);
        assert!(columns.status > 0);
        assert!(columns.size > 0);
        assert!(columns.duration > 0);
        assert!(columns.media > 0);
        assert!(columns.source > 0);
    }

    #[test]
    fn narrow_lists_degrade_to_the_url() {
        let columns = Columns::for_width(20);
        assert_eq!(columns.method, 0);
        assert_eq!(columns.url, 20);
    }
}
