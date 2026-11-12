//! Keymap tests: every shortcut, its bounds and its confirmation rules.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use netra_core::domain::ExchangeFilter;
use netra_tui::app::{App, AppAction, FocusPane, InspectorTab};
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
fn navigation_is_clamped_to_the_visible_rows() {
    let mut app = demo_app();
    assert_eq!(app.state().selection, 0);
    app.on_key(key(KeyCode::Char('k')));
    assert_eq!(
        app.state().selection,
        0,
        "k must not go below the first row"
    );
    for _ in 0..20 {
        app.on_key(key(KeyCode::Char('j')));
    }
    assert_eq!(
        app.state().selection,
        app.visible_rows().len() - 1,
        "j must stop at the last row"
    );
    app.on_key(key(KeyCode::Char('g')));
    assert_eq!(app.state().selection, 0);
    app.on_key(key(KeyCode::Char('G')));
    assert_eq!(app.state().selection, app.visible_rows().len() - 1);
}

#[test]
fn arrows_move_the_selection_like_j_and_k() {
    let mut app = demo_app();
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.state().selection, 1);
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.state().selection, 0);
}

#[test]
fn enter_focuses_the_inspector_and_tab_cycles_panes_and_tabs() {
    let mut app = demo_app();
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.state().focus, FocusPane::Inspector);
    assert_eq!(app.state().tab, InspectorTab::Overview);

    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.state().tab, InspectorTab::RequestHeaders);
    for _ in 0..6 {
        app.on_key(key(KeyCode::Tab));
    }
    assert_eq!(app.state().focus, FocusPane::List);
    assert_eq!(app.state().tab, InspectorTab::Overview);

    app.on_key(key(KeyCode::BackTab));
    assert_eq!(app.state().focus, FocusPane::Inspector);
    assert_eq!(app.state().tab, InspectorTab::Cookies);
    app.on_key(key(KeyCode::BackTab));
    assert_eq!(app.state().tab, InspectorTab::Timing);
}

#[test]
fn digits_select_inspector_tabs() {
    let mut app = demo_app();
    app.on_key(key(KeyCode::Char('5')));
    assert_eq!(app.state().focus, FocusPane::Inspector);
    assert_eq!(app.state().tab, InspectorTab::Response);
    app.on_key(key(KeyCode::Char('0')));
    assert_eq!(app.state().tab, InspectorTab::Response, "0 is not a tab");
    app.on_key(key(KeyCode::Char('9')));
    assert_eq!(app.state().tab, InspectorTab::Response, "9 is not a tab");
}

#[test]
fn search_edits_the_text_filter_and_holds_the_keyboard() {
    let mut app = demo_app();
    assert_eq!(app.on_key(key(KeyCode::Char('/'))), None);
    assert!(app.state().search_active);

    for character in "users".chars() {
        assert_eq!(
            app.on_key(key(KeyCode::Char(character))),
            Some(AppAction::FilterChanged)
        );
    }
    assert_eq!(app.state().search, "users");
    assert_eq!(app.filter().text.as_deref(), Some("users"));
    assert_eq!(app.visible_rows().len(), 1);

    assert_eq!(
        app.on_key(key(KeyCode::Char('q'))),
        Some(AppAction::FilterChanged),
        "q must type into the filter, not quit"
    );
    assert_eq!(app.state().search, "usersq");
    assert_eq!(
        app.on_key(key(KeyCode::Backspace)),
        Some(AppAction::FilterChanged)
    );
    assert_eq!(app.state().search, "users");

    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Some(AppAction::FilterChanged)
    );
    assert!(!app.state().search_active);
    assert_eq!(app.filter().text.as_deref(), Some("users"));
}

#[test]
fn escaping_the_search_restores_the_previous_filter() {
    let mut app = demo_app();
    app.set_filter(ExchangeFilter {
        text: Some("orders".to_owned()),
        ..ExchangeFilter::default()
    });
    app.on_key(key(KeyCode::Char('/')));
    app.on_key(key(KeyCode::Char('x')));
    assert_eq!(app.visible_rows().len(), 0);
    app.on_key(key(KeyCode::Esc));
    assert!(!app.state().search_active);
    assert_eq!(app.filter().text.as_deref(), Some("orders"));
    assert_eq!(app.visible_rows().len(), 1);
}

#[test]
fn clear_view_resets_the_view_but_keeps_the_rows() {
    let mut app = demo_app();
    let rows_before = app.state().rows.clone();
    app.on_key(key(KeyCode::Char('G')));
    app.on_key(key(KeyCode::Char('5')));
    app.on_key(key(KeyCode::Char('f')));
    app.on_key(key(KeyCode::Char('F')));
    app.set_filter(ExchangeFilter {
        text: Some("products".to_owned()),
        ..ExchangeFilter::default()
    });
    assert!(!app.filter().is_empty());

    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        Some(AppAction::ClearView)
    );
    assert_eq!(app.state().rows, rows_before, "rows must survive a clear");
    assert!(app.filter().is_empty());
    assert_eq!(app.state().selection, 0);
    assert_eq!(app.state().tab, InspectorTab::Overview);
    assert_eq!(app.state().focus, FocusPane::List);
    assert!(!app.state().filter_panel_visible);
    assert_eq!(app.state().search, "");
}

#[test]
fn replay_requires_the_confirmation_dialog() {
    let mut app = demo_app();
    let selected = app.selected_id();
    assert!(selected.is_some());
    let id = selected.unwrap_or_default();

    assert_eq!(app.on_key(key(KeyCode::Char('r'))), None);
    let dialog = app.state().confirm.clone();
    assert!(dialog.is_some(), "r must open the confirmation dialog");

    assert_eq!(app.on_key(key(KeyCode::Char('n'))), None);
    assert!(app.state().confirm.is_none());
    assert_eq!(
        app.selected_id(),
        selected,
        "cancelling keeps the selection"
    );

    app.on_key(key(KeyCode::Char('r')));
    assert_eq!(
        app.on_key(key(KeyCode::Char('y'))),
        Some(AppAction::Replay(id))
    );
    assert!(app.state().confirm.is_none());
}

#[test]
fn replay_without_a_selection_opens_nothing() {
    let mut app = App::new();
    assert_eq!(app.on_key(key(KeyCode::Char('r'))), None);
    assert!(app.state().confirm.is_none());
}

#[test]
fn delete_requires_the_confirmation_dialog() {
    let mut app = demo_app();
    assert_eq!(app.on_key(key(KeyCode::Char('d'))), None);
    assert!(app.state().confirm.is_some());
    assert_eq!(
        app.on_key(key(KeyCode::Char('y'))),
        Some(AppAction::DeleteSession)
    );

    app.on_key(key(KeyCode::Char('d')));
    assert_eq!(app.on_key(key(KeyCode::Esc)), None);
    assert!(app.state().confirm.is_none());
    assert_eq!(app.on_key(key(KeyCode::Char('y'))), None);
}

#[test]
fn confirmation_dialog_swallows_navigation_keys() {
    let mut app = demo_app();
    app.on_key(key(KeyCode::Char('d')));
    assert_eq!(app.on_key(key(KeyCode::Char('j'))), None);
    assert_eq!(app.state().selection, 0);
    assert!(app.state().confirm.is_some());
}

#[test]
fn session_actions_are_reported_as_actions() {
    let mut app = demo_app();
    assert_eq!(
        app.on_key(key(KeyCode::Char('s'))),
        Some(AppAction::SaveSession)
    );
    assert_eq!(
        app.on_key(key(KeyCode::Char('p'))),
        Some(AppAction::TogglePause)
    );
    assert_eq!(
        app.on_key(key(KeyCode::Char('e'))),
        Some(AppAction::ExportHar)
    );
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), Some(AppAction::Quit));
}

#[test]
fn help_toggles_and_escape_closes_overlays() {
    let mut app = demo_app();
    assert!(!app.state().help_visible);
    app.on_key(key(KeyCode::Char('?')));
    assert!(app.state().help_visible);
    app.on_key(key(KeyCode::Esc));
    assert!(!app.state().help_visible);

    app.on_key(key(KeyCode::Char('f')));
    assert!(app.state().filter_panel_visible);
    app.on_key(key(KeyCode::Esc));
    assert!(!app.state().filter_panel_visible);
}

#[test]
fn only_failed_is_toggled_by_shift_f() {
    let mut app = demo_app();
    assert_eq!(
        app.on_key(key(KeyCode::Char('F'))),
        Some(AppAction::FilterChanged)
    );
    assert!(app.filter().only_failed);
    assert_eq!(app.visible_rows().len(), 1);
    app.on_key(key(KeyCode::Char('F')));
    assert!(!app.filter().only_failed);
    assert_eq!(app.visible_rows().len(), 5);
}

#[test]
fn needs_detail_tracks_the_selection_and_the_loaded_detail() {
    let mut app = demo_app();
    assert!(app.needs_detail(), "no detail loaded yet");
    let details = demo::details();
    let first = details.first().cloned();
    app.set_detail(first);
    assert!(!app.needs_detail());
    app.on_key(key(KeyCode::Char('j')));
    assert!(app.needs_detail(), "selection moved to another exchange");
    app.set_detail(None);
    assert!(app.needs_detail());
}

#[test]
fn apply_snapshot_keeps_the_filter_and_clamps_the_selection() {
    let mut app = demo_app();
    app.on_key(key(KeyCode::Char('G')));
    app.on_key(key(KeyCode::Char('F')));
    app.apply_snapshot(demo::snapshot());
    assert!(app.filter().only_failed);
    assert!(app.state().selection < app.visible_rows().len());
}

#[test]
fn control_modified_shortcuts_are_ignored() {
    let mut app = demo_app();
    let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(app.on_key(ctrl_q), None);
}
