use azbridge_broker::{
    AppState, app, auth, reconciler,
    settings::{Args, Settings},
};
use clap::Parser;
use std::time::Duration;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "azbridge_broker=info".into()),
        )
        .init();
    let settings = Settings::from_args(Args::parse())?;
    settings.validate_static_dir()?;
    let listener = tokio::net::TcpListener::bind(settings.listen_addr).await?;
    tracing::info!(address = %listener.local_addr()?, dev = settings.dev, "broker listening");
    let state = AppState::initialize(settings).await?;
    let (stop, stopped) = watch::channel(false);
    let reconciler = tokio::spawn(reconciler::run_reconciler(state.clone(), stopped.clone()));
    let cleanup = tokio::spawn(run_auth_cleanup(state.clone(), stopped));
    let server_result = axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown())
        .await;
    stop.send_replace(true);
    let (reconciler_result, cleanup_result) = tokio::join!(reconciler, cleanup);
    server_result?;
    reconciler_result?;
    cleanup_result?;
    Ok(())
}

async fn run_auth_cleanup(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { return; }
            }
            _ = interval.tick() => {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { return; }
                    }
                    result = auth::cleanup_expired_enrollments(&state) => {
                        if result.is_err() {
                            tracing::warn!("expired authentication record cleanup failed");
                        }
                    }
                }
            }
        }
    }
}

async fn shutdown() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "unable to install Ctrl-C handler");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "unable to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("broker shutdown requested");
}

#[cfg(test)]
mod tests {
    use super::*;
    use azbridge_broker::{models::PLATFORM_PARTITION, storage::Table};
    use serde_json::json;

    fn dev_state() -> AppState {
        AppState::new(
            Settings::from_args(Args::try_parse_from(["broker", "--dev"]).unwrap()).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn cleanup_runs_on_start_and_stops_on_shutdown() {
        let state = dev_state();
        state
            .metadata_store
            .insert(
                Table::Enrollments,
                PLATFORM_PARTITION,
                "expired",
                json!({"expiresAt": 0}),
            )
            .await
            .unwrap();
        let (stop, stopped) = watch::channel(false);
        let worker = tokio::spawn(run_auth_cleanup(state.clone(), stopped));
        tokio::time::timeout(Duration::from_secs(2), async {
            while state
                .metadata_store
                .get(Table::Enrollments, PLATFORM_PARTITION, "expired")
                .await
                .unwrap()
                .is_some()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn cleanup_stops_when_sender_is_dropped() {
        let (stop, stopped) = watch::channel(false);
        drop(stop);
        tokio::time::timeout(
            Duration::from_secs(2),
            run_auth_cleanup(dev_state(), stopped),
        )
        .await
        .unwrap();
    }
}
