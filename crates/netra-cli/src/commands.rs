//! Implementations of the non-interactive commands.

use std::{
    io::{self, Write},
    path::PathBuf,
};

use netra_application::{
    AppEvent, ApplicationError, CaptureRuntime, QueryService, SessionService, export::to_har_string,
};
use netra_capture::demo::DemoCaptureProvider;
use netra_core::{
    domain::{CaptureSession, ExchangeFilter, SessionId, SourceSelection},
    ports::CaptureProvider,
};

use crate::{
    app::Bootstrap,
    args::{CaptureArgs, ConfigArgs, DeleteArgs, ExportArgs, ListArgs, RenameArgs, SaveArgs},
    exit::ExitCode,
};

/// Runs `netra sessions list`.
pub async fn sessions_list(bootstrap: &Bootstrap, args: &ListArgs) -> ExitCode {
    let sessions = SessionService::new(bootstrap.context.clone());
    let listed = match sessions.list().await {
        Ok(listed) => listed,
        Err(error) => return fail(&error),
    };
    if args.json {
        match serde_json::to_string_pretty(&listed) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("netra: cannot encode sessions: {error}");
                return ExitCode::Failure;
            }
        }
        return ExitCode::Success;
    }
    if listed.is_empty() {
        println!("no sessions yet; run `netra capture --demo` to try one");
        return ExitCode::Success;
    }
    println!(
        "{:<10} {:<20} {:<10} {:<6} {:>8} {:>7}  CREATED",
        "ID", "NAME", "STATUS", "SOURCE", "REQUESTS", "ERRORS"
    );
    for session in listed {
        println!(
            "{:<10} {:<20} {:<10} {:<6} {:>8} {:>7}  {}",
            session.id.short(),
            truncate(&session.name, 20),
            session.status.as_str(),
            session.source.as_str(),
            session.exchange_count,
            session.error_count,
            session.created_at.to_rfc3339(),
        );
    }
    ExitCode::Success
}

/// Runs `netra sessions rename`.
pub async fn sessions_rename(bootstrap: &Bootstrap, args: &RenameArgs) -> ExitCode {
    let sessions = SessionService::new(bootstrap.context.clone());
    let id = match resolve(&sessions, &args.session_id).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match sessions.rename(id, &args.name).await {
        Ok(session) => {
            println!("renamed {} to {}", session.id.short(), session.name);
            ExitCode::Success
        }
        Err(error) => fail(&error),
    }
}

/// Runs `netra sessions save`.
pub async fn sessions_save(bootstrap: &Bootstrap, args: &SaveArgs) -> ExitCode {
    let sessions = SessionService::new(bootstrap.context.clone());
    let id = match resolve(&sessions, &args.session_id).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    match sessions.save(id).await {
        Ok(session) => {
            println!(
                "saved {}{}",
                session.id.short(),
                session
                    .saved_at
                    .map(|at| format!(" at {}", at.to_rfc3339()))
                    .unwrap_or_default()
            );
            ExitCode::Success
        }
        Err(error) => fail(&error),
    }
}

/// Runs `netra sessions delete`.
pub async fn sessions_delete(bootstrap: &Bootstrap, args: &DeleteArgs) -> ExitCode {
    let sessions = SessionService::new(bootstrap.context.clone());
    let id = match resolve(&sessions, &args.session_id).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let session = match sessions.get(id).await {
        Ok(session) => session,
        Err(error) => return fail(&error),
    };
    if !args.yes
        && !confirm(&format!(
            "delete session {} ({}) and all of its exchanges?",
            session.id.short(),
            session.name
        ))
    {
        println!("aborted");
        return ExitCode::Failure;
    }
    match sessions.delete(id).await {
        Ok(()) => {
            println!("deleted {}", session.id.short());
            ExitCode::Success
        }
        Err(error) => fail(&error),
    }
}

/// Runs `netra sessions export`.
pub async fn sessions_export(bootstrap: &Bootstrap, args: &ExportArgs) -> ExitCode {
    let sessions = SessionService::new(bootstrap.context.clone());
    let queries = QueryService::new(bootstrap.context.clone());
    let id = match resolve(&sessions, &args.session_id).await {
        Ok(id) => id,
        Err(code) => return code,
    };
    let session = match sessions.get(id).await {
        Ok(session) => session,
        Err(error) => return fail(&error),
    };
    let format = match args.format.resolve() {
        Ok(format) => format,
        Err(error) => return fail(&error),
    };
    let exchanges = match collect_exchanges(&queries, id).await {
        Ok(exchanges) => exchanges,
        Err(error) => return fail(&error),
    };
    let text = match format {
        crate::args::ExportFormat::Har => match to_har_string(&session, &exchanges, "netra") {
            Ok(text) => text,
            Err(error) => {
                eprintln!("netra: cannot encode HAR: {error}");
                return ExitCode::Failure;
            }
        },
    };
    let path: PathBuf = args.output.clone().unwrap_or_else(|| {
        bootstrap
            .exports_dir()
            .join(format!("{}.har", session.id.short()))
    });
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("netra: cannot create {}: {error}", parent.display());
        return ExitCode::Failure;
    }
    match std::fs::write(&path, text) {
        Ok(()) => {
            println!(
                "exported {} exchanges of {} to {}",
                exchanges.len(),
                session.id.short(),
                path.display()
            );
            ExitCode::Success
        }
        Err(error) => {
            eprintln!("netra: cannot write {}: {error}", path.display());
            ExitCode::Failure
        }
    }
}

/// Runs `netra config`.
pub fn config(bootstrap: &Bootstrap, args: &ConfigArgs) -> ExitCode {
    if args.json {
        match serde_json::to_string_pretty(&bootstrap.config) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("netra: cannot encode configuration: {error}");
                return ExitCode::Failure;
            }
        }
        return ExitCode::Success;
    }
    println!("config file   {}", bootstrap.config_path.display());
    println!("data dir      {}", bootstrap.config.data_dir.display());
    println!(
        "database      {}",
        bootstrap.config.database_path().display()
    );
    println!("exports       {}", bootstrap.exports_dir().display());
    println!(
        "limits        max body {} bytes, headers {}, exchanges {} per session",
        bootstrap.config.limits.max_body_bytes,
        bootstrap.config.limits.max_headers_per_side,
        bootstrap.config.limits.max_exchanges_per_session
    );
    println!(
        "proxy         {}:{} (tls interception {})",
        bootstrap.config.proxy.bind_addr,
        bootstrap.config.proxy.port,
        if bootstrap.config.proxy.intercept_tls {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!(
        "redaction     headers: {} · query: {} · json fields: {} · body capture {}",
        bootstrap.config.redaction.header_names.len(),
        bootstrap.config.redaction.query_params.len(),
        bootstrap.config.redaction.json_fields.len(),
        if bootstrap.config.redaction.capture_bodies {
            "enabled"
        } else {
            "disabled"
        }
    );
    ExitCode::Success
}

/// Runs `netra capture`.
pub async fn capture(bootstrap: &Bootstrap, args: &CaptureArgs) -> ExitCode {
    let selection: SourceSelection = args.source.into();
    // Providers are resolved before the session row exists: a capture that
    // cannot start must not leave an empty session behind.
    let providers = match providers_for(selection, args.demo) {
        Ok(providers) => providers,
        Err(error) => {
            let code = crate::exit::exit_code_for(&error);
            eprintln!("netra: {error}");
            return code;
        }
    };
    let sessions = SessionService::new(bootstrap.context.clone());
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| format!("capture {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S")));
    let session = match sessions.create(&name, selection).await {
        Ok(session) => session,
        Err(error) => return fail(&error),
    };
    println!(
        "session {} ({}) capturing from {}",
        session.id.short(),
        session.name,
        selection
    );
    let runtime =
        match CaptureRuntime::start(bootstrap.context.clone(), session.clone(), providers).await {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = sessions.delete(session.id).await;
                return fail(&error);
            }
        };
    let mut events = runtime.subscribe();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("stopping capture");
                break;
            }
            event = events.recv() => {
                match event {
                    Ok(AppEvent::Activity { .. }) => {}
                    Ok(AppEvent::Paused { paused, .. }) => {
                        println!("capture {}", if paused { "paused" } else { "resumed" });
                    }
                    Ok(AppEvent::Skipped { count, .. }) => {
                        eprintln!("netra: {count} events discarded while paused");
                    }
                    Ok(AppEvent::Warning { message }) => eprintln!("netra: {message}"),
                    Ok(AppEvent::Stopped { reason, .. }) => {
                        println!("capture stopped: {reason}");
                        break;
                    }
                    Err(_) => break,
                }
            }
        }
    }
    if let Err(error) = runtime.stop().await {
        eprintln!("netra: {error}");
    }
    match sessions.counts(session.id).await {
        Ok(counts) => println!(
            "persisted {} exchanges ({} failed) in session {}",
            counts.total,
            counts.errors,
            session.id.short()
        ),
        Err(error) => eprintln!("netra: cannot count exchanges: {error}"),
    }
    ExitCode::Success
}

/// Builds providers for a source selection.
///
/// Only the demo provider exists before milestones M3 (CDP) and M4 (proxy); the
/// error names that explicitly instead of pretending to capture.
fn providers_for(
    selection: SourceSelection,
    demo: bool,
) -> Result<Vec<Box<dyn CaptureProvider>>, ApplicationError> {
    if demo {
        return Ok(vec![Box::new(DemoCaptureProvider::new())]);
    }
    Err(ApplicationError::Unsupported {
        feature: match selection {
            SourceSelection::Cdp => "live CDP capture (milestone M3)",
            SourceSelection::Proxy => "the HTTP/HTTPS proxy (milestone M4)",
            SourceSelection::Both => "live capture (milestones M3 and M4)",
        },
    })
}

/// Loads every exchange of a session for export.
async fn collect_exchanges(
    queries: &QueryService,
    session_id: SessionId,
) -> Result<Vec<netra_core::domain::NetworkExchange>, ApplicationError> {
    let mut offset = 0;
    let mut collected = Vec::new();
    loop {
        let page = queries
            .list(session_id, &ExchangeFilter::default(), 500, offset)
            .await?;
        let count = page.len();
        for summary in page {
            collected.push(queries.inspect(summary.id).await?);
        }
        if count < 500 {
            break;
        }
        offset += count;
    }
    Ok(collected)
}

/// Resolves a session id or unique prefix, printing the reason on failure.
pub async fn resolve_session(bootstrap: &Bootstrap, raw: &str) -> Result<SessionId, ExitCode> {
    let sessions = SessionService::new(bootstrap.context.clone());
    resolve(&sessions, raw).await
}

/// Resolves a session id or unique prefix.
async fn resolve(sessions: &SessionService, raw: &str) -> Result<SessionId, ExitCode> {
    let trimmed = raw.trim();
    if let Ok(id) = SessionId::parse(trimmed) {
        return match sessions.get(id).await {
            Ok(_) => Ok(id),
            Err(error) => Err(fail(&error)),
        };
    }
    let listed = match sessions.list().await {
        Ok(listed) => listed,
        Err(error) => return Err(fail(&error)),
    };
    let matches: Vec<CaptureSession> = listed
        .into_iter()
        .filter(|summary| summary.id.to_string().starts_with(trimmed))
        .map(|summary| CaptureSession {
            id: summary.id,
            name: summary.name,
            status: summary.status,
            source: summary.source,
            created_at: summary.created_at,
            finished_at: summary.finished_at,
            saved_at: summary.saved_at,
        })
        .collect();
    match matches.len() {
        0 => {
            eprintln!("netra: no session matches `{trimmed}`");
            Err(ExitCode::NotFound)
        }
        1 => Ok(matches[0].id),
        count => {
            eprintln!("netra: `{trimmed}` matches {count} sessions; use a longer prefix");
            Err(ExitCode::Failure)
        }
    }
}

/// Asks for confirmation on stdin.
fn confirm(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn fail(error: &ApplicationError) -> ExitCode {
    eprintln!("netra: {error}");
    crate::exit::exit_code_for(error)
}
