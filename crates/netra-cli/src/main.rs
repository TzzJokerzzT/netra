//! `netra` command-line entry point.

mod app;
mod args;
mod commands;
mod exit;
mod tui;
mod update;

use clap::Parser;
use netra_application::SessionService;

use crate::{
    args::{CaCommand, Cli, Command, SessionsCommand},
    exit::ExitCode,
};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    app::init_logging();
    let cli = Cli::parse();
    let bootstrap = match app::Bootstrap::load(cli.config) {
        Ok(bootstrap) => bootstrap,
        Err(error) => {
            eprintln!("netra: {error}");
            return ExitCode::Failure.into();
        }
    };

    // Crash recovery happens before any command so the session list never shows
    // a session as live when this process is the only one that could update it.
    let sessions = SessionService::new(bootstrap.context.clone());
    let probe = netra_core::ports::SystemProcessProbe;
    match sessions.recover_interrupted(&probe).await {
        Ok(recovered) if !recovered.is_empty() => eprintln!(
            "netra: recovered {} interrupted session(s); their recorded exchanges are intact",
            recovered.len()
        ),
        Ok(_) => {}
        Err(error) => eprintln!("netra: cannot recover interrupted sessions: {error}"),
    }

    // The update notice goes to stderr before the TUI takes the terminal, and
    // never runs for the commands that must stay silent and offline.
    if !stays_offline(&cli.command) {
        update::startup_check(&bootstrap.config);
    }

    let code = match cli.command {
        None => open_latest(&bootstrap).await,
        Some(Command::Capture(args)) => commands::capture(&bootstrap, &args).await,
        Some(Command::Config(args)) => commands::config(&bootstrap, &args),
        Some(Command::Ca(args)) => match args.command {
            CaCommand::Init => commands::ca_init(&bootstrap),
            CaCommand::Export(export) => commands::ca_export(&bootstrap, &export),
            CaCommand::Show => commands::ca_show(&bootstrap),
        },
        Some(Command::Sessions(group)) => match group.command {
            SessionsCommand::List(args) => commands::sessions_list(&bootstrap, &args).await,
            SessionsCommand::Open(args) => open_named(&bootstrap, &args.session_id).await,
            SessionsCommand::Rename(args) => commands::sessions_rename(&bootstrap, &args).await,
            SessionsCommand::Save(args) => commands::sessions_save(&bootstrap, &args).await,
            SessionsCommand::Delete(args) => commands::sessions_delete(&bootstrap, &args).await,
            SessionsCommand::Export(args) => commands::sessions_export(&bootstrap, &args).await,
        },
        Some(Command::Update(args)) => commands::update(&args).await,
    };
    code.into()
}

/// `true` for the commands whose start must not touch the network.
///
/// `--help` and `--version` never reach this code: clap answers them while
/// parsing. The explicit update path must not consult a cache it is about to
/// overwrite, and `netra config` reports the switch instead of acting on it.
fn stays_offline(command: &Option<Command>) -> bool {
    matches!(command, Some(Command::Update(_) | Command::Config(_)))
}

async fn open_latest(bootstrap: &app::Bootstrap) -> ExitCode {
    let sessions = SessionService::new(bootstrap.context.clone());
    let latest = match sessions.list().await {
        Ok(listed) => listed.into_iter().next().map(|summary| summary.id),
        Err(error) => {
            eprintln!("netra: cannot list sessions: {error}");
            return ExitCode::Failure;
        }
    };
    if latest.is_none() {
        eprintln!("netra: no sessions yet; run `netra capture --demo` to record a synthetic one");
    }
    tui::run(bootstrap, latest, None).await
}

async fn open_named(bootstrap: &app::Bootstrap, raw: &str) -> ExitCode {
    match commands::resolve_session(bootstrap, raw).await {
        Ok(id) => tui::run(bootstrap, Some(id), None).await,
        Err(code) => code,
    }
}
