// Bridge modules are scaffolded; suppress dead_code until host orchestrator wires them.
#![allow(dead_code)]

mod cli;
mod config;
mod config_loader;
mod host;
mod http_forward;
mod http_remote_forwarder;
mod logging;
mod preamble;
mod remote_bridge;
mod service;
#[cfg(unix)]
mod socket;
mod stream_pump;
mod tcp;
mod udp;

use clap::Parser;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse CLI arguments
    let cli = cli::Cli::parse();

    // Handle Windows Service commands
    #[cfg(windows)]
    {
        if cli.svc_install {
            return service::windows::install_service();
        }
        if cli.svc_uninstall {
            return service::windows::uninstall_service();
        }
        if cli.svc {
            return service::windows::run_as_service();
        }
    }

    #[cfg(not(windows))]
    {
        if cli.svc_install || cli.svc_uninstall || cli.svc {
            anyhow::bail!(
                "Windows Service commands (-I, -U, --svc) are only supported on Windows"
            );
        }
    }

    // Load configuration (merges machine/user/file/CLI layers)
    let config = config_loader::load_config(&cli)?;
    config.validate()?;

    // Determine log level: CLI flags override config
    let log_level = if cli.quiet {
        Some("QUIET")
    } else if cli.verbose >= 3 {
        Some("DEBUG3")
    } else if cli.verbose >= 2 {
        Some("DEBUG")
    } else if cli.verbose >= 1 {
        Some("VERBOSE")
    } else {
        config.log_level.as_deref()
    };

    // Determine log file: CLI flag overrides config
    let log_file_path;
    let log_file = if let Some(ref path) = cli.log_file {
        log_file_path = path.clone();
        Some(log_file_path.as_path())
    } else if let Some(ref name) = config.log_file_name {
        log_file_path = std::path::PathBuf::from(name);
        Some(log_file_path.as_path())
    } else {
        None
    };

    // Initialize logging (guard must live until program exit for file logging)
    let _log_guard = logging::init(log_level, log_file);

    // Validate at least one forward is configured
    if config.local_forward.is_empty() && config.remote_forward.is_empty() {
        anyhow::bail!("No forwarding rules configured. Use -L, -T, or -H to specify at least one.");
    }

    // Validate connection string or endpoint.
    // A global connection is not required when every forward entry carries its
    // own ConnectionString.
    let all_forwards_have_cs =
        config.local_forward.iter().all(|lf| lf.connection_string.is_some())
        && config.remote_forward.iter().all(|rf| rf.connection_string.is_some());

    if config.azure_relay_connection_string.is_none()
        && config.azure_relay_endpoint.is_none()
        && !all_forwards_have_cs
    {
        anyhow::bail!(
            "No Azure Relay connection configured. Use -x (connection string) or -e (endpoint URI)."
        );
    }

    info!(
        local_forwards = config.local_forward.len(),
        remote_forwards = config.remote_forward.len(),
        "Starting Azure Relay Bridge"
    );

    // Start the host
    let host = std::sync::Arc::new(host::Host::new(config));
    host.start().await?;

    info!("Azure Relay Bridge is running. Press Ctrl+C to stop.");

    // Wait for shutdown signal (Ctrl+C or SIGTERM on Unix)
    let host_shutdown = host.clone();
    shutdown_signal().await;
    info!("Shutdown signal received, stopping...");

    host_shutdown.stop().await;
    info!("Azure Relay Bridge stopped.");
    Ok(())
}

/// Wait for a shutdown signal (Ctrl+C on all platforms, SIGTERM on Unix).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }
}
