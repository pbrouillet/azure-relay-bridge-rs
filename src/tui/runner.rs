use std::path::PathBuf;
use std::sync::Arc;
use anyhow::Result;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::host::Host;
use super::log_layer::SharedLogSender;

/// Status of the running bridge instance.
#[derive(Debug, Clone, PartialEq)]
pub enum RunStatus {
    Idle,
    Starting,
    Running,
    Stopped,
    Error(String),
}

/// Manages a bridge Host instance running in a background task.
pub struct Runner {
    pub status: RunStatus,
    pub logs: Vec<String>,
    log_rx: Option<mpsc::UnboundedReceiver<String>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    shared_sender: SharedLogSender,
}

impl Runner {
    pub fn new(shared_sender: SharedLogSender) -> Self {
        Self {
            status: RunStatus::Idle,
            logs: Vec::new(),
            log_rx: None,
            shutdown_tx: None,
            shared_sender,
        }
    }

    /// Start a bridge host from a config file.
    pub async fn start(&mut self, config_path: PathBuf) -> Result<()> {
        self.stop().await;
        self.logs.clear();
        self.status = RunStatus::Starting;

        let content = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(e) => {
                self.status = RunStatus::Error(format!("Failed to read config: {e}"));
                return Ok(());
            }
        };

        let config: Config = match serde_yaml::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                self.status = RunStatus::Error(format!("Failed to parse config: {e}"));
                return Ok(());
            }
        };

        if let Err(e) = config.validate() {
            self.status = RunStatus::Error(format!("Config validation failed: {e}"));
            return Ok(());
        }

        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        self.log_rx = Some(log_rx);
        self.shutdown_tx = Some(shutdown_tx);

        // Install the sender into the shared slot so the ChannelLayer
        // routes tracing events into our channel.
        {
            let mut guard = self.shared_sender.lock().unwrap();
            *guard = Some(log_tx.clone());
        }

        let shared_sender = self.shared_sender.clone();
        let log_tx_clone = log_tx.clone();
        tokio::spawn(async move {
            let _ = log_tx.send(format!("Starting bridge with config: {}", config_path.display()));

            let host = Arc::new(Host::new(config));
            match host.start().await {
                Ok(()) => {
                    let _ = log_tx_clone.send("Bridge started successfully.".to_string());
                }
                Err(e) => {
                    let _ = log_tx_clone.send(format!("Bridge start failed: {e}"));
                    // Clear the shared sender on failure
                    let mut guard = shared_sender.lock().unwrap();
                    *guard = None;
                    return;
                }
            }

            let _ = log_tx_clone.send("Bridge is running. Press 'q' to stop.".to_string());

            // Wait for shutdown signal
            let _ = shutdown_rx.recv().await;

            let _ = log_tx_clone.send("Shutting down bridge...".to_string());
            host.stop().await;
            let _ = log_tx_clone.send("Bridge stopped.".to_string());

            // Clear the shared sender after shutdown
            let mut guard = shared_sender.lock().unwrap();
            *guard = None;
        });

        self.status = RunStatus::Running;
        Ok(())
    }

    /// Stop the running bridge instance.
    pub async fn stop(&mut self) {
        // Clear the shared sender so the ChannelLayer stops routing events
        {
            let mut guard = self.shared_sender.lock().unwrap();
            *guard = None;
        }

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
        self.log_rx = None;
        if self.status == RunStatus::Running || self.status == RunStatus::Starting {
            self.status = RunStatus::Stopped;
        }
    }

    /// Poll for new log messages from the background task.
    pub async fn poll_logs(&mut self) {
        if let Some(ref mut rx) = self.log_rx {
            while let Ok(msg) = rx.try_recv() {
                self.logs.push(msg);
            }
        }
    }
}
