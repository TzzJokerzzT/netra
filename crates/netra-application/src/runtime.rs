//! The capture runtime: providers in, persisted exchanges out.
//!
//! ```text
//! providers ──(bounded mpsc)──> consumer ──> processor ──> repositories
//!                                   │
//!                                   └──(broadcast)──> UI notifications
//! ```
//!
//! Properties this module is responsible for:
//!
//! * Capture never blocks the renderer: providers run in their own tasks and
//!   the UI only receives notifications, reading rows from storage itself.
//! * Backpressure instead of loss: a provider awaits on a full bounded channel.
//! * No silent drops: skipped (paused), failed writes and retention deletes are
//!   reported as [`AppEvent`]s.
//! * Crash safety: the session row is kept in `capturing`/`paused` while the
//!   process runs, so the next start can recover it.

use netra_capture::CaptureProcessor;
use netra_core::{
    domain::{CaptureEvent, CaptureSession, SessionId, Timestamp},
    ports::CaptureProvider,
};
use tokio::{
    sync::{broadcast, mpsc, watch},
    task::JoinHandle,
};

use crate::{ApplicationError, ApplicationResult, context::AppContext};

/// Capacity of the provider to consumer channel.
pub const EVENT_CHANNEL_CAPACITY: usize = 1024;
/// Capacity of the UI broadcast channel.
pub const NOTIFICATION_CAPACITY: usize = 256;
/// Applied updates between retention sweeps.
const RETENTION_INTERVAL: u64 = 500;
/// Skipped events between pause notifications.
const SKIPPED_NOTIFICATION_INTERVAL: u64 = 100;
/// How long shutdown waits for tasks before aborting them.
const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Notification sent to the UI when capture state changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    /// New data was persisted; reload the visible window.
    Activity {
        /// Session that changed.
        session_id: SessionId,
    },
    /// Capture stopped.
    Stopped {
        /// Session that stopped.
        session_id: SessionId,
        /// Why it stopped.
        reason: String,
    },
    /// Pause state changed.
    Paused {
        /// Session in question.
        session_id: SessionId,
        /// Whether capture is now paused.
        paused: bool,
    },
    /// Events were discarded because capture was paused.
    Skipped {
        /// Session in question.
        session_id: SessionId,
        /// Number of discarded events.
        count: u64,
    },
    /// A non-fatal problem worth surfacing.
    Warning {
        /// Human-readable message.
        message: String,
    },
}

/// Control messages accepted by the consumer task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Pause,
    Resume,
    Stop,
}

/// Handle to a running capture.
#[derive(Debug)]
pub struct CaptureRuntime {
    session_id: SessionId,
    control: mpsc::Sender<Control>,
    shutdown: watch::Sender<bool>,
    notifications: broadcast::Sender<AppEvent>,
    providers: Vec<JoinHandle<()>>,
    consumer: JoinHandle<()>,
}

impl CaptureRuntime {
    /// Starts capturing for `session` with the supplied providers.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Conflict`] when no provider was supplied and
    /// [`ApplicationError::Storage`] when the session cannot be marked active.
    pub async fn start(
        context: AppContext,
        session: CaptureSession,
        mut providers: Vec<Box<dyn CaptureProvider>>,
    ) -> ApplicationResult<Self> {
        if providers.is_empty() {
            return Err(ApplicationError::Conflict(
                "no capture provider is available for this source selection".to_owned(),
            ));
        }
        let session_id = session.id;
        let (event_sender, event_receiver) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let (control_sender, control_receiver) = mpsc::channel(8);
        let (notification_sender, _) = broadcast::channel(NOTIFICATION_CAPACITY);
        let (shutdown_sender, shutdown_receiver) = watch::channel(false);

        let mut provider_tasks = Vec::with_capacity(providers.len());
        for provider in providers.drain(..) {
            provider_tasks.push(spawn_provider(
                provider,
                event_sender.clone(),
                shutdown_receiver.clone(),
            ));
        }
        drop(event_sender);
        drop(shutdown_receiver);

        let consumer = spawn_consumer(
            context.clone(),
            session,
            event_receiver,
            control_receiver,
            notification_sender.clone(),
        );

        Ok(Self {
            session_id,
            control: control_sender,
            shutdown: shutdown_sender,
            notifications: notification_sender,
            providers: provider_tasks,
            consumer,
        })
    }

    /// Session being captured.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Subscribes to capture notifications.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.notifications.subscribe()
    }

    /// Stops recording without stopping the providers.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Conflict`] when the runtime already stopped.
    pub async fn pause(&self) -> ApplicationResult<()> {
        self.control
            .send(Control::Pause)
            .await
            .map_err(|_| ApplicationError::Conflict("capture already stopped".to_owned()))
    }

    /// Records again after a pause.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Conflict`] when the runtime already stopped.
    pub async fn resume(&self) -> ApplicationResult<()> {
        self.control
            .send(Control::Resume)
            .await
            .map_err(|_| ApplicationError::Conflict("capture already stopped".to_owned()))
    }

    /// Stops capture, waits for in-flight writes and stops every provider.
    ///
    /// # Errors
    ///
    /// Returns [`ApplicationError::Conflict`] when the runtime already stopped.
    pub async fn stop(mut self) -> ApplicationResult<()> {
        let _ = self.shutdown.send(true);
        let _ = self.control.send(Control::Stop).await;
        join_with_timeout(&mut self.consumer).await;
        for task in &mut self.providers {
            join_with_timeout(task).await;
        }
        self.providers.clear();
        Ok(())
    }
}

impl Drop for CaptureRuntime {
    fn drop(&mut self) {
        // Dropping without `stop` models a crash: capture tasks are aborted and
        // the session row stays active so the next start recovers it.
        let _ = self.shutdown.send(true);
        self.consumer.abort();
        for task in &self.providers {
            task.abort();
        }
    }
}

async fn join_with_timeout(task: &mut JoinHandle<()>) {
    let finished = tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut *task).await;
    if finished.is_err() {
        task.abort();
    }
}

fn spawn_provider(
    mut provider: Box<dyn CaptureProvider>,
    sender: mpsc::Sender<(netra_core::domain::CaptureSource, CaptureEvent)>,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    let source = provider.source();
    tokio::spawn(async move {
        if let Err(error) = provider.start().await {
            tracing::error!(source = %source, error = %error, "capture provider failed to start");
            return;
        }
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    let _ = changed;
                    if provider.stop().await.is_err() {
                        tracing::warn!(source = %source, "capture provider failed to stop cleanly");
                    }
                    return;
                }
                event = provider.next_event() => {
                    match event {
                        Ok(Some(event)) => {
                            if sender.send((source, event)).await.is_err() {
                                return;
                            }
                        }
                        Ok(None) => return,
                        Err(error) => {
                            tracing::warn!(source = %source, error = %error, "capture provider failed");
                            return;
                        }
                    }
                }
            }
        }
    })
}

fn spawn_consumer(
    context: AppContext,
    session: CaptureSession,
    mut events: mpsc::Receiver<(netra_core::domain::CaptureSource, CaptureEvent)>,
    mut control: mpsc::Receiver<Control>,
    notifications: broadcast::Sender<AppEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let session_id = session.id;
        let mut processor = CaptureProcessor::new(
            session_id,
            context.config.limits.clone(),
            context.config.redaction.clone(),
        );
        let mut paused = false;
        let mut skipped: u64 = 0;
        let mut applied: u64 = 0;
        let stop_reason: String;

        if let Err(error) = context.sessions.update(&session).await {
            tracing::error!(error = %error, "cannot mark session active");
        }
        // Record the owning process so another `netra` command never mistakes
        // this live capture for a crashed one.
        if let Err(error) = context
            .sessions
            .set_owner(session_id, std::process::id())
            .await
        {
            tracing::error!(error = %error, "cannot record session owner");
        }

        loop {
            tokio::select! {
                biased;
                message = control.recv() => {
                    match message {
                        Some(Control::Pause) => {
                            paused = true;
                            notify_paused(&context, &notifications, &session_id, true).await;
                        }
                        Some(Control::Resume) => {
                            paused = false;
                            notify_paused(&context, &notifications, &session_id, false).await;
                        }
                        Some(Control::Stop) | None => {
                            stop_reason = "stopped by user".to_owned();
                            break;
                        }
                    }
                }
                event = events.recv() => {
                    match event {
                        None => {
                            stop_reason = "all capture sources finished".to_owned();
                            break;
                        }
                        Some((_source, event)) => {
                            if paused {
                                skipped += 1;
                                if skipped % SKIPPED_NOTIFICATION_INTERVAL == 0 {
                                    let _ = notifications.send(AppEvent::Skipped {
                                        session_id,
                                        count: skipped,
                                    });
                                }
                                continue;
                            }
                            match event {
                                CaptureEvent::SourceDisconnected { source, reason } => {
                                    let _ = notifications.send(AppEvent::Warning {
                                        message: format!("{source:?} capture disconnected: {reason}"),
                                    });
                                    continue;
                                }
                                CaptureEvent::SourceReconnected { source } => {
                                    let _ = notifications.send(AppEvent::Warning {
                                        message: format!("{source:?} capture reconnected"),
                                    });
                                    continue;
                                }
                                other => {
                                    let updates = processor.apply(other);
                                    if updates.is_empty() {
                                        continue;
                                    }
                                    for update in &updates {
                                        applied += 1;
                                        if let Err(error) = context.exchanges.apply(update).await {
                                            tracing::error!(error = %error, "cannot persist exchange update");
                                            let _ = notifications.send(AppEvent::Warning {
                                                message: format!("storage write failed: {error}"),
                                            });
                                        }
                                    }
                                    let _ = notifications.send(AppEvent::Activity { session_id });
                                    if applied % RETENTION_INTERVAL == 0 {
                                        let keep = context.config.limits.max_exchanges_per_session;
                                        match context.exchanges.enforce_retention(session_id, keep).await {
                                            Ok(0) => {}
                                            Ok(deleted) => {
                                                let _ = notifications.send(AppEvent::Warning {
                                                    message: format!(
                                                        "retention removed {deleted} old exchanges (limit {keep})"
                                                    ),
                                                });
                                            }
                                            Err(error) => {
                                                let _ = notifications.send(AppEvent::Warning {
                                                    message: format!("retention sweep failed: {error}"),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut finished = session;
        finished.stop(Timestamp::now());
        if let Err(error) = context.sessions.update(&finished).await {
            tracing::error!(error = %error, "cannot mark session stopped");
        }
        let _ = notifications.send(AppEvent::Stopped {
            session_id,
            reason: stop_reason,
        });
    })
}

async fn notify_paused(
    context: &AppContext,
    notifications: &broadcast::Sender<AppEvent>,
    session_id: &SessionId,
    paused: bool,
) {
    let mut session = match context.sessions.get(*session_id).await {
        Ok(Some(session)) => session,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(error = %error, "cannot load session for pause");
            return;
        }
    };
    if paused {
        session.pause();
    } else {
        session.resume();
    }
    if let Err(error) = context.sessions.update(&session).await {
        tracing::error!(error = %error, "cannot persist pause state");
    }
    let _ = notifications.send(AppEvent::Paused {
        session_id: *session_id,
        paused,
    });
}
