//! Filtering tests: the supplied rows are narrowed by `ExchangeFilter` only.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use netra_core::domain::{ExchangeFilter, HttpMethod, StatusRange};
use netra_tui::app::App;
use netra_tui::demo;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn demo_app() -> App {
    let mut app = App::new();
    app.apply_snapshot(demo::snapshot());
    app
}

#[test]
fn text_filter_narrows_the_visible_rows() {
    let mut app = demo_app();
    assert_eq!(app.visible_rows().len(), 5);
    app.set_filter(ExchangeFilter {
        text: Some("orders".to_owned()),
        ..ExchangeFilter::default()
    });
    assert_eq!(app.visible_rows().len(), 1);
    assert!(
        app.visible_rows()
            .iter()
            .all(|row| row.url.as_str().contains("orders"))
    );
}

#[test]
fn text_filter_ignores_case() {
    let mut app = demo_app();
    app.set_filter(ExchangeFilter {
        text: Some("ORDERS".to_owned()),
        ..ExchangeFilter::default()
    });
    assert_eq!(app.visible_rows().len(), 1);
}

#[test]
fn method_filter_narrows_the_visible_rows() {
    let mut app = demo_app();
    let methods: Vec<HttpMethod> = ["POST"]
        .iter()
        .filter_map(|method| HttpMethod::parse(method).ok())
        .collect();
    app.set_filter(ExchangeFilter {
        methods,
        ..ExchangeFilter::default()
    });
    assert_eq!(app.visible_rows().len(), 1);
    assert_eq!(app.visible_rows()[0].method.as_str(), "POST");
}

#[test]
fn status_filter_narrows_the_visible_rows() {
    let mut app = demo_app();
    app.set_filter(ExchangeFilter {
        status: StatusRange::parse("2xx").unwrap_or_default(),
        ..ExchangeFilter::default()
    });
    assert_eq!(app.visible_rows().len(), 2);
    assert!(
        app.visible_rows()
            .iter()
            .all(|row| row.status.is_some_and(|status| status.is_success()))
    );
}

#[test]
fn server_error_filter_selects_the_failing_status() {
    let mut app = demo_app();
    app.set_filter(ExchangeFilter {
        status: StatusRange::parse("5xx").unwrap_or_default(),
        ..ExchangeFilter::default()
    });
    assert_eq!(app.visible_rows().len(), 1);
    assert_eq!(
        app.visible_rows()[0].status.map(|status| status.as_u16()),
        Some(500)
    );
}

#[test]
fn only_failed_combines_with_status_filters() {
    let mut app = demo_app();
    app.set_filter(ExchangeFilter {
        status: StatusRange::parse("2xx").unwrap_or_default(),
        ..ExchangeFilter::default()
    });
    app.on_key(key(KeyCode::Char('F')));
    assert!(
        app.visible_rows().is_empty(),
        "only-failed combined with 2xx matches nothing"
    );
    app.on_key(key(KeyCode::Char('c')));
    app.on_key(key(KeyCode::Char('F')));
    assert_eq!(app.visible_rows().len(), 1);
}

#[test]
fn selection_clamps_when_the_filter_shrinks_the_list() {
    let mut app = demo_app();
    app.on_key(key(KeyCode::Char('G')));
    assert_eq!(app.state().selection, 4);
    app.set_filter(ExchangeFilter {
        text: Some("orders".to_owned()),
        ..ExchangeFilter::default()
    });
    assert_eq!(app.visible_rows().len(), 1);
    assert_eq!(app.state().selection, 0);
}

#[test]
fn clearing_the_filter_restores_every_row() {
    let mut app = demo_app();
    app.set_filter(ExchangeFilter {
        text: Some("nothing-matches-this".to_owned()),
        ..ExchangeFilter::default()
    });
    assert!(app.visible_rows().is_empty());
    app.on_key(key(KeyCode::Char('c')));
    assert_eq!(app.visible_rows().len(), 5);
    assert!(app.filter().is_empty());
}

#[test]
fn demo_snapshot_rows_are_never_mutated_by_filters() {
    let mut app = demo_app();
    let before = app.state().rows.clone();
    app.set_filter(ExchangeFilter {
        text: Some("orders".to_owned()),
        ..ExchangeFilter::default()
    });
    app.on_key(key(KeyCode::Char('c')));
    assert_eq!(app.state().rows, before);
}
