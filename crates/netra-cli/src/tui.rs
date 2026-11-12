//! Interactive terminal session browser.
//!
//! The TUI only reads through the application layer: capture, storage and
//! rendering are separate, so a refresh, a filter change or a tab switch can
//! never drop captured data. While a `netra capture` process writes to the same
//! database, WAL keeps both sides consistent.

use std::{
    io::{self, Stdout},
    path::PathBuf,
    time::Duration,
};

use crossterm::{
    event::{Event, KeyEventKind, poll, read},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use netra_application::{
    AppEvent, CaptureRuntime, QueryService, SessionService, export::to_har_string,
};
use netra_core::domain::{ExchangeFilter, SessionId};
use netra_tui::{App, AppAction, InspectorData, SessionHeader, Snapshot};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::{app::Bootstrap, exit::ExitCode};

/// Redraw and refresh interval.
const TICK: Duration = Duration::from_millis(200);

/// Runs the TUI until the user quits.
pub async fn run(
    bootstrap: &Bootstrap,
    session_id: Option<SessionId>,
    runtime: Option<CaptureRuntime>,
) -> ExitCode {
    let mut terminal = match TerminalSession::new() {
        Ok(session) => session,
        Err(error) => {
            eprintln!("netra: cannot start the terminal UI: {error}");
            return ExitCode::Failure;
        }
    };

    let mut app = App::new();
    let mut runtime = runtime;
    let mut session_id = session_id;
    let (key_sender, mut keys) = mpsc::channel(64);
    std::thread::spawn(move || {
        loop {
            match poll(Duration::from_millis(100)) {
                Ok(true) => match read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                        if key_sender.blocking_send(key).is_err() {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => return,
                },
                Ok(false) => {}
                Err(_) => return,
            }
        }
    });

    let mut notifications = runtime.as_ref().map(CaptureRuntime::subscribe);
    let mut ticker = tokio::time::interval(TICK);
    let mut outcome = ExitCode::Success;

    loop {
        refresh(&mut app, bootstrap, session_id).await;
        if let Err(error) = terminal.terminal.draw(|frame| app.render(frame)) {
            eprintln!("netra: cannot draw: {error}");
            outcome = ExitCode::Failure;
            break;
        }

        tokio::select! {
            _ = ticker.tick() => {}
            key = keys.recv() => {
                let Some(key) = key else {
                    break;
                };
                if let Some(action) = app.on_key(key) {
                    match handle_action(action, &mut app, bootstrap, &mut session_id, &mut runtime).await {
                        ActionOutcome::Continue => {}
                        ActionOutcome::Quit => break,
                    }
                }
            }
            event = async {
                match notifications.as_mut() {
                    Some(receiver) => receiver.recv().await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                if let Some(event) = event {
                    note_runtime_event(&mut app, &event);
                }
            }
        }
    }

    if let Some(runtime) = runtime.take()
        && let Err(error) = runtime.stop().await
    {
        eprintln!("netra: {error}");
    }
    drop(terminal);
    outcome
}

/// Terminal guard that always restores the previous screen.
struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;
        terminal.clear()?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

enum ActionOutcome {
    Continue,
    Quit,
}

async fn handle_action(
    action: AppAction,
    app: &mut App,
    bootstrap: &Bootstrap,
    session_id: &mut Option<SessionId>,
    runtime: &mut Option<CaptureRuntime>,
) -> ActionOutcome {
    match action {
        AppAction::Quit => return ActionOutcome::Quit,
        AppAction::SaveSession => {
            if let Some(id) = session_id {
                let sessions = SessionService::new(bootstrap.context.clone());
                match sessions.save(*id).await {
                    Ok(session) => app.set_status(format!(
                        "session saved at {}",
                        session
                            .saved_at
                            .map(|at| at.to_rfc3339())
                            .unwrap_or_default()
                    )),
                    Err(error) => app.set_status(format!("cannot save session: {error}")),
                }
            } else {
                app.set_status("no session to save");
            }
        }
        AppAction::TogglePause => match runtime.as_ref() {
            Some(runtime) => {
                let paused = app.state().paused;
                let result = if paused {
                    runtime.resume().await
                } else {
                    runtime.pause().await
                };
                match result {
                    Ok(()) => app.set_paused(!paused),
                    Err(error) => app.set_status(format!("cannot change capture: {error}")),
                }
            }
            None => app.set_status("capture is not running in this process; use `netra capture`"),
        },
        AppAction::DeleteSession => match session_id {
            Some(id) => {
                let sessions = SessionService::new(bootstrap.context.clone());
                match sessions.delete(*id).await {
                    Ok(()) => {
                        app.set_status(format!("deleted session {}", id.short()));
                        *session_id = None;
                    }
                    Err(error) => app.set_status(format!("cannot delete session: {error}")),
                }
            }
            None => app.set_status("no session to delete"),
        },
        AppAction::ExportHar => {
            if let Some(id) = session_id {
                match export_har(bootstrap, *id).await {
                    Ok(path) => app.set_status(format!("exported to {}", path.display())),
                    Err(error) => app.set_status(format!("cannot export: {error}")),
                }
            } else {
                app.set_status("no session to export");
            }
        }
        AppAction::Replay(_) => {
            app.set_status("replay is not implemented yet (milestone M6)");
        }
        AppAction::Refresh | AppAction::FilterChanged | AppAction::ClearView => {}
    }
    ActionOutcome::Continue
}

fn note_runtime_event(app: &mut App, event: &AppEvent) {
    match event {
        AppEvent::Paused { paused, .. } => app.set_paused(*paused),
        AppEvent::Warning { message } => app.set_status(message.clone()),
        AppEvent::Skipped { count, .. } => {
            app.set_status(format!("{count} events discarded while paused"));
        }
        AppEvent::Stopped { reason, .. } => app.set_status(format!("capture stopped: {reason}")),
        AppEvent::Activity { .. } => {}
    }
}

async fn refresh(app: &mut App, bootstrap: &Bootstrap, session_id: Option<SessionId>) {
    let queries = QueryService::new(bootstrap.context.clone());
    let Some(id) = session_id else {
        app.apply_snapshot(Snapshot {
            header: SessionHeader {
                session_id: None,
                name: "no session".to_owned(),
                status: "idle".to_owned(),
                sources: "-".to_owned(),
                total: 0,
                errors: 0,
                elapsed: netra_core::domain::DurationMs::ZERO,
            },
            rows: Vec::new(),
        });
        return;
    };
    let sessions = SessionService::new(bootstrap.context.clone());
    let Ok(session) = sessions.get(id).await else {
        return;
    };
    let counts = queries.counts(id).await.unwrap_or_default();
    let filter = app.filter().clone();
    let rows = queries.recent(id, &filter).await.unwrap_or_default();
    app.apply_snapshot(Snapshot {
        header: SessionHeader {
            session_id: Some(id),
            name: session.name.clone(),
            status: if app.state().paused {
                "paused".to_owned()
            } else {
                session.status.as_str().to_owned()
            },
            sources: session.source.as_str().to_owned(),
            total: counts.total,
            errors: counts.errors,
            elapsed: session.duration(bootstrap.context.clock.now()),
        },
        rows,
    });
    if app.needs_detail()
        && let Some(selected) = app.selected_id()
        && let Ok(exchange) = queries.inspect(selected).await
    {
        app.set_detail(Some(InspectorData { exchange }));
    }
}

async fn export_har(bootstrap: &Bootstrap, session_id: SessionId) -> Result<PathBuf, String> {
    let sessions = SessionService::new(bootstrap.context.clone());
    let queries = QueryService::new(bootstrap.context.clone());
    let session = sessions
        .get(session_id)
        .await
        .map_err(|error| error.to_string())?;
    let mut exchanges = Vec::new();
    let mut offset = 0;
    loop {
        let page = queries
            .list(session_id, &ExchangeFilter::default(), 500, offset)
            .await
            .map_err(|error| error.to_string())?;
        let count = page.len();
        for summary in page {
            exchanges.push(
                queries
                    .inspect(summary.id)
                    .await
                    .map_err(|error| error.to_string())?,
            );
        }
        if count < 500 {
            break;
        }
        offset += count;
    }
    let text = to_har_string(&session, &exchanges, "netra")?;
    let directory = bootstrap.exports_dir();
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("{}.har", session_id.short()));
    std::fs::write(&path, text).map_err(|error| error.to_string())?;
    Ok(path)
}
