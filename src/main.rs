// Backstops "no panics in request-handling code" at compile time. Scoped
// off during test builds (`cfg(test)`), where `.unwrap()`/`.expect()` on a
// known-good fixture is normal and clearer than threading `Result` through
// every assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod auth;
mod config;
mod error;
mod events;
mod health;
mod hostenv;
mod journal;
mod logging;
mod quadlet;
mod systemd;
mod web;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Notify, broadcast};
use tracing::{error, info};

use config::{AppState, Config};

fn main() -> anyhow::Result<()> {
    // The --hash-password helper runs synchronously, before any async
    // runtime or logging is set up -- it's a one-shot CLI utility, not part
    // of the server.
    if std::env::args().nth(1).as_deref() == Some("--hash-password") {
        return hash_password_cli();
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run())
}

/// Prompts for a password on stderr (not echoed) and prints an argon2 hash
/// to stdout, suitable for `SOOTH_AUTH_PASSWORD_HASH`. Exists so operators
/// never need a separate tool just to configure the one credential this app
/// has.
fn hash_password_cli() -> anyhow::Result<()> {
    let password = rpassword::prompt_password("Password: ")?;
    let confirm = rpassword::prompt_password("Confirm password: ")?;
    if password != confirm {
        anyhow::bail!("passwords did not match");
    }
    println!("{}", auth::hash_password(&password)?);
    Ok(())
}

async fn run() -> anyhow::Result<()> {
    let config_path = std::env::var_os("SOOTH_CONFIG")
        .map(Into::into)
        .unwrap_or_else(config::default_config_path);
    let config = Config::load(Some(config_path.clone()))?;
    logging::init(config.log_filter.as_deref());
    config.validate()?;

    let quadlet_dir = config.resolved_quadlet_dir()?;
    quadlet::discovery::ensure_dir(&quadlet_dir)?;
    info!(path = %quadlet_dir.display(), "using quadlet directory");

    info!("connecting to the systemd user session bus");
    let systemd_client = systemd::Client::connect().await?;

    let initial_health = health::Health::check();
    if !initial_health.podman_generator_found {
        tracing::warn!(
            "podman's quadlet generator was not found on this host; \
             create/edit will skip dry-run validation against it"
        );
    }
    if initial_health.linger_enabled == Some(false) {
        tracing::warn!(
            "linger is not enabled for this user; sooth and the units it manages \
             will stop on logout unless `loginctl enable-linger` is run"
        );
    }
    let health = health::HealthCell::new(initial_health);

    let (events_tx, _rx) = broadcast::channel(256);

    // Live status updates: forward systemd PropertiesChanged signals onto
    // the dashboard's event channel.
    let watch_task = systemd::watch::spawn(
        systemd_client.clone(),
        events_tx.clone(),
        quadlet_dir.clone(),
    )
    .await?;

    // External-edit detection: watch the quadlet directory itself, debounce
    // bursts (editors write-then-rename), then reload systemd and notify the
    // UI that the unit list may have changed.
    let (fs_changed_tx, mut fs_changed_rx) = broadcast::channel(16);
    let _watcher = quadlet::discovery::watch(&quadlet_dir, fs_changed_tx)?;
    let fs_watch_task = {
        let systemd_client = systemd_client.clone();
        let events_tx = events_tx.clone();
        tokio::spawn(async move {
            loop {
                if fs_changed_rx.recv().await.is_err() {
                    break;
                }
                // Debounce: collapse a burst of events (e.g. an editor's
                // write-then-rename) into a single reload + refresh.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                while fs_changed_rx.try_recv().is_ok() {}

                if let Err(e) = systemd_client.reload().await {
                    error!(error = %e, "failed to reload systemd after external quadlet change");
                }
                let _ = events_tx.send(events::DashboardEvent::UnitsChanged);
            }
        })
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let restart = Arc::new(Notify::new());

    let state = AppState {
        config: Arc::new(config.clone()),
        quadlet_dir: Arc::new(quadlet_dir),
        systemd: Arc::new(systemd_client),
        health,
        config_path: Arc::new(config_path),
        events: events_tx,
        shutdown: shutdown_rx,
        restart: restart.clone(),
    };

    let app = web::build_router(state);
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    info!(addr = %config.bind_addr, "sooth listening");

    // Set by `shutdown_signal` when the wind-down was triggered by the
    // Settings "Restart" button rather than a real signal.
    let restart_requested = Arc::new(AtomicBool::new(false));

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(
            shutdown_tx,
            restart,
            restart_requested.clone(),
        ))
        .await?;

    // Both of these loop forever on their own (there's nothing that closes
    // their channel/connection), so they must be cancelled explicitly here --
    // otherwise the still-running tasks block `Runtime::drop` in `main`
    // forever, and the process never actually exits despite having already
    // logged that it shut down.
    watch_task.abort();
    fs_watch_task.abort();

    if restart_requested.load(Ordering::SeqCst) {
        // The listener is dropped by now, so the port is free for the fresh
        // process to rebind. `exec` only returns on failure.
        return reexec();
    }

    info!("shut down cleanly");
    Ok(())
}

/// Replace the current process with a fresh `sooth`, inheriting argv and the
/// environment. This is how the in-app "Restart" button reloads config: it
/// works the same whether sooth runs under systemd, the dev script, or a
/// bare shell, none of which a `systemctl restart` or a plain exit would.
#[cfg(unix)]
fn reexec() -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe()?;
    info!(exe = %exe.display(), "restarting: re-executing");
    let err = std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .exec();
    Err(anyhow::anyhow!(
        "failed to re-exec {}: {err}",
        exe.display()
    ))
}

#[cfg(not(unix))]
fn reexec() -> anyhow::Result<()> {
    anyhow::bail!("in-app restart is only supported on Unix")
}

/// Waits for Ctrl-C, SIGTERM, or the Settings page's "Restart" button, then
/// flips `shutdown_tx` before returning. `with_graceful_shutdown` uses this
/// future's completion to start winding down (stop accepting new
/// connections, wait for in-flight ones); the flag flip is what lets our own
/// SSE handlers notice and end their otherwise-infinite streams so that wait
/// actually finishes. See `AppState::shutdown`. A restart sets
/// `restart_requested` so `main` re-execs instead of exiting.
async fn shutdown_signal(
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    restart: Arc<Notify>,
    restart_requested: Arc<AtomicBool>,
) {
    let ctrl_c = async {
        // SIGINT (Ctrl-C from a terminal, or `systemctl --user stop`'s
        // initial signal) is always available; SIGTERM matters for the
        // systemd-service deployment path.
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!(error = %e, "failed to install Ctrl-C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => error!(error = %e, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("shutdown signal received"),
        _ = terminate => info!("shutdown signal received"),
        _ = restart.notified() => {
            info!("restart requested from the Settings page");
            restart_requested.store(true, Ordering::SeqCst);
        }
    }
    let _ = shutdown_tx.send(true);
}
