use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Handles graceful shutdown on SIGTERM and SIGINT signals.
///
/// Spawns a background task that listens for shutdown signals and triggers
/// a cancellation token when received.
pub struct SigDown {
    _task_tracker: TaskTracker,
    cancellation_token: CancellationToken,
}

impl SigDown {
    /// Creates a new signal handler.
    ///
    /// Returns an error if signal registration fails.
    pub fn try_new() -> Result<Self, std::io::Error> {
        let inner = CancellationToken::new();
        let outer = inner.clone();
        let task_tracker = TaskTracker::new();
        
        task_tracker.spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                
                let mut sigterm = match signal(SignalKind::terminate()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Failed to register SIGTERM handler: {}", e);
                        return;
                    }
                };
                let mut sigint = match signal(SignalKind::interrupt()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Failed to register SIGINT handler: {}", e);
                        return;
                    }
                };

                tokio::select! {
                    _ = sigterm.recv() => {
                        inner.cancel();
                    },
                    _ = sigint.recv() => {
                        inner.cancel();
                    }
                }
            }

            #[cfg(not(unix))]
            {
                match tokio::signal::ctrl_c().await {
                    Ok(()) => {
                        inner.cancel();
                    },
                    Err(err) => {
                        eprintln!("Unable to listen for shutdown signal: {}", err);
                    },
                }
            }
        });
        
        task_tracker.close();
        Ok(Self {
            _task_tracker: task_tracker,
            cancellation_token: outer,
        })
    }

    /// Returns a clone of the cancellation token for distributing to subsystems.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    /// Waits for a shutdown signal and ensures the signal handler task completes.
    #[allow(dead_code)]
    pub async fn recv(&self) {
        self.cancellation_token.cancelled().await;
        self._task_tracker.wait().await;
    }
}
