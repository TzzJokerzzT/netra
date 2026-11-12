//! The inspector: tab bar plus tab-specific content panels.

use netra_analysis::{body_view::BodyView, errors, timing::Waterfall};
use netra_core::domain::{
    Body, HttpHeaders, NetworkExchange, human_bytes, redaction::REDACTION_MARKER,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

use super::{fit, truncate};
use crate::app::{InspectorData, InspectorTab};

/// Maximum payload prefix rendered as text.
const MAX_BODY_BYTES: usize = 4096;
/// Columns used by waterfall bars.
const BAR_COLUMNS: usize = 32;

/// Renders the tab bar and the active tab panel.
pub fn render(
    frame: &mut Frame<'_>,
    area: Rect,
    detail: Option<&InspectorData>,
    tab: InspectorTab,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let tab_lines = tab_bar(tab, usize::from(area.width));
    let tab_height = u16::try_from(tab_lines.len())
        .unwrap_or(u16::MAX)
        .min(area.height);
    let chunks = Layout::vertical([Constraint::Length(tab_height), Constraint::Min(0)]).split(area);
    frame.render_widget(Paragraph::new(tab_lines), chunks[0]);

    let block = Block::bordered()
        .title(panel_title(detail, tab))
        .border_style(Style::default().fg(Color::DarkGray));
    let paragraph = Paragraph::new(panel_lines(detail, tab))
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, chunks[1]);
}

/// Lays the tab chips out over as many lines as the width requires.
fn tab_bar(active: InspectorTab, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for tab in InspectorTab::ALL {
        let chip = format!(" {} {} ", tab.number(), tab.label());
        let chip_width = chip.chars().count();
        if used > 0 && used.saturating_add(chip_width) > width {
            lines.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
        }
        let style = if tab == active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(chip, style));
        used = used.saturating_add(chip_width);
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

/// Block title for the active tab, including body state for payload tabs.
#[must_use]
pub fn panel_title(detail: Option<&InspectorData>, tab: InspectorTab) -> String {
    let Some(detail) = detail else {
        return format!("{} · no exchange selected", tab.label());
    };
    let exchange = &detail.exchange;
    match tab {
        InspectorTab::Payload => format!("Payload · {}", exchange.request.body.describe()),
        InspectorTab::Response => match &exchange.response {
            Some(response) => format!("Response · {}", response.body.describe()),
            None => "Response · no response received".to_owned(),
        },
        _ => tab.label().to_owned(),
    }
}

/// Content lines of the active tab.
fn panel_lines(detail: Option<&InspectorData>, tab: InspectorTab) -> Vec<Line<'static>> {
    let Some(detail) = detail else {
        return vec![
            dim("No exchange selected."),
            dim("Move with j/k, then press Enter to inspect."),
        ];
    };
    let exchange = &detail.exchange;
    match tab {
        InspectorTab::Overview => overview_lines(exchange),
        InspectorTab::RequestHeaders => {
            header_lines(&exchange.request.headers, "no request headers captured")
        }
        InspectorTab::ResponseHeaders => match &exchange.response {
            Some(response) => header_lines(&response.headers, "no response headers captured"),
            None => vec![dim("no response received yet")],
        },
        InspectorTab::Payload => body_lines(&exchange.request.body),
        InspectorTab::Response => match &exchange.response {
            Some(response) => body_lines(&response.body),
            None => vec![dim("no response received yet")],
        },
        InspectorTab::Timing => timing_lines(exchange),
        InspectorTab::Cookies => cookie_lines(exchange),
    }
}

/// Method, URL, state, sizes, duration and the error insight.
fn overview_lines(exchange: &NetworkExchange) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            exchange.request.method.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::raw(exchange.request.url.as_str().to_owned()),
    ])];
    let status = match (&exchange.response, exchange.status()) {
        (Some(response), Some(status)) => match response.status_text.as_deref() {
            Some(text) if !text.is_empty() => format!("{status} {text}"),
            _ => status.to_string(),
        },
        _ => "no response yet".to_owned(),
    };
    lines.push(field("status", status));
    lines.push(field("state", exchange.state.as_str()));
    lines.push(field("source", exchange.source.as_str()));
    lines.push(field("request body", exchange.request.body.describe()));
    lines.push(field(
        "response body",
        exchange.response.as_ref().map_or_else(
            || "no response".to_owned(),
            |response| response.body.describe(),
        ),
    ));
    lines.push(field("size", human_bytes(exchange.size_bytes())));
    lines.push(field(
        "duration",
        exchange.duration().map_or_else(
            || "not measured".to_owned(),
            |duration| duration.to_string(),
        ),
    ));
    if let Some(address) = exchange
        .response
        .as_ref()
        .and_then(|response| response.remote_address.as_deref())
    {
        lines.push(field("remote", address));
    }
    if let Some(version) = exchange.request.http_version.as_deref() {
        lines.push(field("http", version));
    }
    if let Some(insight) = errors::insight(exchange) {
        lines.push(Line::from(Span::styled(
            format!("error: {}", insight.summary),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(Span::styled(
            format!("check: {}", insight.suggestion),
            Style::default().fg(Color::Yellow),
        )));
    }
    lines
}

/// Ordered header lines, marking values withheld by the redaction policy.
fn header_lines(headers: &HttpHeaders, empty: &str) -> Vec<Line<'static>> {
    if headers.is_empty() {
        return vec![dim(empty)];
    }
    headers
        .iter()
        .map(|entry| {
            let redacted = entry.value == REDACTION_MARKER;
            let value_style = if redacted {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC)
            } else {
                Style::default()
            };
            let mut spans = vec![
                Span::styled(
                    format!("{}: ", entry.name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(entry.value.clone(), value_style),
            ];
            if redacted {
                spans.push(Span::styled(
                    "  (redacted by policy)",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

/// Payload lines chosen by [`BodyView`], never a raw binary dump.
fn body_lines(body: &Body) -> Vec<Line<'static>> {
    match BodyView::from_body(body, MAX_BODY_BYTES) {
        BodyView::Empty { description } => vec![dim(description)],
        BodyView::Text { text, truncated } => text_lines(&text, truncated),
        BodyView::Json {
            text,
            valid,
            error,
            truncated,
        } => {
            let mut lines = text_lines(&text, truncated);
            if !valid {
                lines.push(Line::from(Span::styled(
                    format!("invalid JSON: {}", error.unwrap_or_default()),
                    Style::default().fg(Color::Red),
                )));
            }
            lines
        }
        BodyView::Hex {
            rows,
            shown_bytes,
            total_bytes,
        } => {
            let mut lines = vec![dim(format!(
                "binary payload, showing {shown_bytes} of {} bytes",
                human_bytes(total_bytes)
            ))];
            lines.extend(rows.into_iter().map(Line::from));
            lines
        }
    }
}

/// Splits rendered payload text into display lines.
fn text_lines(text: &str, truncated: bool) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = text
        .lines()
        .map(|line| Line::from(line.to_owned()))
        .collect();
    if lines.is_empty() {
        lines.push(dim("empty payload"));
    }
    if truncated {
        lines.push(dim("body truncated at the capture limit"));
    }
    lines
}

/// Waterfall bars; an explicit notice when the source measured nothing.
fn timing_lines(exchange: &NetworkExchange) -> Vec<Line<'static>> {
    let waterfall = Waterfall::from_exchange(exchange);
    if waterfall.is_empty() {
        return vec![dim("no timing data measured for this exchange")];
    }
    let mut lines: Vec<Line<'static>> = waterfall
        .segments
        .iter()
        .map(|segment| {
            let start = ratio_to_columns(segment.start_ratio);
            let width = ratio_to_columns(segment.width_ratio).max(1);
            let bar = format!("{}{}", " ".repeat(start), "#".repeat(width));
            Line::from(vec![
                Span::styled(fit(segment.label, 9), Style::default().fg(Color::DarkGray)),
                Span::raw(fit(&segment.duration.to_string(), 8)),
                Span::styled(
                    truncate(&bar, BAR_COLUMNS),
                    Style::default().fg(Color::Cyan),
                ),
            ])
        })
        .collect();
    lines.push(field("total", waterfall.total.to_string()));
    lines
}

/// Converts a waterfall ratio into whole columns.
fn ratio_to_columns(ratio: f64) -> usize {
    if !ratio.is_finite() || ratio <= 0.0 {
        return 0;
    }
    let columns = (ratio * BAR_COLUMNS as f64).round();
    if columns <= 0.0 {
        return 0;
    }
    (columns as usize).min(BAR_COLUMNS)
}

/// Cookie name/value rows from `Cookie` and `Set-Cookie` headers.
fn cookie_lines(exchange: &NetworkExchange) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for value in exchange.request.headers.get_all("cookie") {
        for (name, value) in cookie_pairs(value) {
            lines.push(cookie_row("request", &name, &value, ""));
        }
    }
    if let Some(response) = &exchange.response {
        for header in response.headers.get_all("set-cookie") {
            let (pair, attributes) = match header.split_once(';') {
                Some((pair, attributes)) => (pair, attributes.trim()),
                None => (header, ""),
            };
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            lines.push(cookie_row(
                "response",
                name.trim(),
                value.trim(),
                attributes,
            ));
        }
    }
    if lines.is_empty() {
        lines.push(dim("no cookies in this exchange"));
    }
    lines
}

/// Parses `a=1; b=2` into pairs.
fn cookie_pairs(value: &str) -> Vec<(String, String)> {
    value
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            Some((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

/// One cookie row tagged with its origin.
fn cookie_row(origin: &str, name: &str, value: &str, attributes: &str) -> Line<'static> {
    let mut spans = vec![
        Span::styled(fit(origin, 9), Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{name} = "), Style::default().fg(Color::Cyan)),
        Span::raw(value.to_owned()),
    ];
    if !attributes.is_empty() {
        spans.push(Span::styled(
            format!("  [{attributes}]"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

/// A dim informational line.
fn dim(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(Color::DarkGray),
    ))
}

/// A `name: value` field line.
fn field(name: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{name}: "), Style::default().fg(Color::DarkGray)),
        Span::raw(value.into()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use netra_core::domain::{
        CaptureSource, ContentType, ExchangeState, HttpMethod, NetworkRequest, NetworkResponse,
        OmitReason, RequestUrl, SessionId, StatusCode, Timestamp,
    };

    fn exchange() -> NetworkExchange {
        let request = NetworkRequest::new(
            HttpMethod::parse("GET").unwrap(),
            RequestUrl::new("https://example.com/a?b=1"),
            Timestamp::from_unix_millis(0),
        );
        NetworkExchange::pending(SessionId::new(), CaptureSource::Cdp, request, None)
    }

    fn detail(exchange: NetworkExchange) -> InspectorData {
        InspectorData { exchange }
    }

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn cookie_pairs_split_on_semicolons() {
        assert_eq!(
            cookie_pairs("session=abc; theme=dark"),
            vec![
                ("session".to_owned(), "abc".to_owned()),
                ("theme".to_owned(), "dark".to_owned())
            ]
        );
        assert!(cookie_pairs("  ").is_empty());
    }

    #[test]
    fn cookies_come_from_request_and_response_headers() {
        let mut exchange = exchange();
        exchange
            .request
            .headers
            .push("Cookie", "session=abc; theme=dark");
        let mut response = NetworkResponse::new(
            StatusCode::try_new(200).unwrap(),
            Timestamp::from_unix_millis(10),
        );
        response
            .headers
            .push("Set-Cookie", "token=xyz; Path=/; HttpOnly");
        exchange.complete(response);
        let rendered = text(&cookie_lines(&exchange));
        assert!(rendered.contains("request"));
        assert!(rendered.contains("session = abc"));
        assert!(rendered.contains("theme = dark"));
        assert!(rendered.contains("token = xyz"));
        assert!(rendered.contains("Path=/"));
    }

    #[test]
    fn timing_without_measurements_says_so() {
        let rendered = text(&timing_lines(&exchange()));
        assert!(rendered.contains("no timing data"));
    }

    #[test]
    fn timing_bars_follow_the_measured_ratios() {
        let mut exchange = exchange();
        exchange.timing = Some(netra_core::domain::TimingMetrics {
            dns: Some(netra_core::domain::DurationMs::from_millis(10)),
            ttfb: Some(netra_core::domain::DurationMs::from_millis(30)),
            total: Some(netra_core::domain::DurationMs::from_millis(40)),
            ..netra_core::domain::TimingMetrics::default()
        });
        let rendered = text(&timing_lines(&exchange));
        assert!(rendered.contains("dns"));
        assert!(rendered.contains('#'));
        assert!(rendered.contains("total: 40 ms"));
    }

    #[test]
    fn payload_titles_report_the_body_state() {
        let omitted = detail({
            let mut exchange = exchange();
            exchange.request.body = Body::omitted(
                Some(ContentType::parse("image/png")),
                OmitReason::TooLarge,
                4096,
            );
            exchange
        });
        assert!(panel_title(Some(&omitted), InspectorTab::Payload).contains("too large"));

        let redacted = detail({
            let mut exchange = exchange();
            exchange.request.body =
                Body::redacted(Some(ContentType::parse("application/json")), 64);
            exchange
        });
        assert!(panel_title(Some(&redacted), InspectorTab::Payload).contains("redacted"));

        let absent = detail(exchange());
        assert!(panel_title(Some(&absent), InspectorTab::Payload).contains("absent"));
        assert!(panel_title(Some(&absent), InspectorTab::RequestHeaders).contains("Req headers"));
    }

    #[test]
    fn redacted_header_values_are_marked() {
        let mut exchange = exchange();
        exchange
            .request
            .headers
            .push("Authorization", REDACTION_MARKER);
        let rendered = text(&header_lines(
            &exchange.request.headers,
            "no request headers captured",
        ));
        assert!(rendered.contains("Authorization: [REDACTED]"));
        assert!(rendered.contains("(redacted by policy)"));
    }

    #[test]
    fn overview_reports_errors_for_failed_statuses() {
        let mut exchange = exchange();
        exchange.complete(NetworkResponse::new(
            StatusCode::try_new(500).unwrap(),
            Timestamp::from_unix_millis(5),
        ));
        let rendered = text(&overview_lines(&exchange));
        assert!(rendered.contains("status: 500"));
        assert!(rendered.contains("server error (500)"));
    }

    #[test]
    fn aborted_exchanges_stay_visible_in_the_overview() {
        let mut exchange = exchange();
        exchange.abort(Some("navigation".to_owned()));
        assert_eq!(exchange.state, ExchangeState::Aborted);
        let rendered = text(&overview_lines(&exchange));
        assert!(rendered.contains("state: aborted"));
        assert!(rendered.contains("aborted"));
    }
}
