//! Rendering tests: the screen is rendered into a `TestBackend` buffer and the
//! visible text is asserted, so no terminal is needed.

use netra_tui::app::App;
use netra_tui::demo;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Position};

/// Renders the app and returns every visible cell as text.
fn render_text(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = match Terminal::new(TestBackend::new(width, height)) {
        Ok(terminal) => terminal,
        Err(_) => return String::new(),
    };
    let _ = terminal.draw(|frame| app.render(frame));
    buffer_text(terminal.backend().buffer())
}

/// Flattens a buffer into one string per row.
fn buffer_text(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let position = Position {
                x: area.x.saturating_add(x),
                y: area.y.saturating_add(y),
            };
            out.push_str(buffer.cell(position).map_or(" ", |cell| cell.symbol()));
        }
        out.push('\n');
    }
    out
}

/// App showing the synthetic session with its first detail loaded.
fn demo_app() -> App {
    let mut app = App::new();
    app.apply_snapshot(demo::snapshot());
    app.set_detail(demo::details().into_iter().next());
    app
}

#[test]
fn wide_layout_shows_list_and_inspector_side_by_side() {
    let mut app = demo_app();
    let screen = render_text(&mut app, 120, 30);
    assert!(screen.contains("netra"), "header badge missing:\n{screen}");
    assert!(screen.contains("demo session"), "session name missing");
    assert!(screen.contains("capturing"), "status missing");
    assert!(screen.contains("elapsed 1m05s"), "elapsed missing");
    assert!(screen.contains("METHOD"), "list header missing");
    assert!(screen.contains("api.example.com"), "host missing");
    assert!(screen.contains("/v1/products"), "path missing");
    assert!(screen.contains("Overview"), "inspector tab bar missing");
    assert!(
        screen.contains("status: 200 OK"),
        "overview content missing"
    );
    assert!(screen.contains("? help"), "status line hint missing");
}

#[test]
fn narrow_layout_shows_only_the_list_until_the_inspector_is_opened() {
    let mut app = demo_app();
    let list_only = render_text(&mut app, 60, 20);
    assert!(list_only.contains("METHOD"));
    assert!(!list_only.contains("status: 200 OK"));

    app.on_key(key(crossterm::event::KeyCode::Enter));
    let inspector = render_text(&mut app, 60, 20);
    assert!(inspector.contains("Overview"));
    assert!(inspector.contains("status: 200 OK"));
    assert!(!inspector.contains("METHOD"), "list should be hidden");
}

#[test]
fn every_inspector_tab_renders_its_content() {
    let mut app = demo_app();
    for (digit, expected) in [
        ('1', "product"),
        ('2', "Accept: application/json"),
        ('3', "Content-Type: application/json"),
        ('4', "no payload"),
        ('5', "items"),
        ('6', "dns"),
        ('7', "no cookies in this exchange"),
    ] {
        app.on_key(key(crossterm::event::KeyCode::Char(digit)));
        let screen = render_text(&mut app, 120, 30);
        assert!(
            screen.contains(expected),
            "tab {digit} did not render {expected:?}:\n{screen}"
        );
    }
}

#[test]
fn response_tab_reports_a_redacted_body() {
    let mut app = App::new();
    app.apply_snapshot(demo::snapshot());
    let details = demo::details();
    let redacted = details.into_iter().find(|detail| {
        detail
            .exchange
            .response
            .as_ref()
            .is_some_and(|response| response.body.bytes().is_none())
    });
    app.set_detail(redacted);
    app.on_key(key(crossterm::event::KeyCode::Char('5')));
    let screen = render_text(&mut app, 120, 30);
    assert!(
        screen.contains("redacted"),
        "panel title must state redaction"
    );
}

#[test]
fn help_overlay_is_visible_after_pressing_question_mark() {
    let mut app = demo_app();
    app.on_key(key(crossterm::event::KeyCode::Char('?')));
    let screen = render_text(&mut app, 120, 30);
    assert!(screen.contains("Help"));
    assert!(screen.contains("move the selection"));
    assert!(screen.contains("Esc closes the overlay"));
}

#[test]
fn filter_panel_lists_the_active_filter() {
    let mut app = demo_app();
    app.on_key(key(crossterm::event::KeyCode::Char('f')));
    app.on_key(key(crossterm::event::KeyCode::Char('F')));
    let screen = render_text(&mut app, 120, 30);
    assert!(screen.contains("Filters"));
    assert!(screen.contains("only failed: yes"));
}

#[test]
fn confirm_dialog_shows_the_pending_action() {
    let mut app = demo_app();
    app.on_key(key(crossterm::event::KeyCode::Char('d')));
    let screen = render_text(&mut app, 120, 30);
    assert!(screen.contains("Confirm"));
    assert!(screen.contains("Delete this session"));
    assert!(screen.contains("cancel"));
}

#[test]
fn tiny_terminals_get_a_message_instead_of_a_broken_layout() {
    let mut app = demo_app();
    let screen = render_text(&mut app, 30, 10);
    assert!(screen.contains("Terminal too small"));
    assert!(screen.contains("netra needs at least 40x12"));
}

#[test]
fn every_small_size_renders_without_panicking() {
    let mut app = demo_app();
    for (width, height) in [
        (1, 1),
        (5, 3),
        (20, 8),
        (30, 10),
        (39, 11),
        (40, 12),
        (99, 12),
        (100, 12),
        (160, 50),
    ] {
        let screen = render_text(&mut app, width, height);
        assert!(!screen.is_empty(), "{width}x{height} rendered nothing");
    }
}

#[test]
fn tiny_terminals_are_safe_with_overlays_open() {
    let mut app = demo_app();
    app.on_key(key(crossterm::event::KeyCode::Char('?')));
    app.on_key(key(crossterm::event::KeyCode::Char('f')));
    app.on_key(key(crossterm::event::KeyCode::Char('d')));
    for (width, height) in [(30, 10), (40, 12), (45, 13)] {
        let screen = render_text(&mut app, width, height);
        assert!(!screen.is_empty());
    }
}

#[test]
fn no_selection_renders_an_explicit_empty_state() {
    let mut app = App::new();
    app.apply_snapshot(demo::snapshot());
    app.on_key(key(crossterm::event::KeyCode::Enter));
    let screen = render_text(&mut app, 120, 30);
    assert!(screen.contains("No exchange selected"));
    assert!(screen.contains("no exchange selected"));
}

fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
}

#[test]
fn set_status_text_appears_in_the_status_line() {
    let mut app = demo_app();
    app.set_status("session saved to disk");
    let screen = render_text(&mut app, 120, 30);
    assert!(
        screen.contains("session saved to disk"),
        "status text missing:\n{screen}"
    );
}

#[test]
fn paused_state_is_shown_in_the_header() {
    let mut app = demo_app();
    assert!(!app.state().paused);
    app.set_paused(true);
    assert!(app.state().paused);
    let screen = render_text(&mut app, 120, 30);
    assert!(
        screen.contains("paused"),
        "header must show the paused status:\n{screen}"
    );

    app.set_paused(false);
    assert!(!app.state().paused);
    let screen = render_text(&mut app, 120, 30);
    assert!(screen.contains("capturing"));
    assert!(!screen.contains("paused"));
}
