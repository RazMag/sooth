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
mod imageinfo;
mod journal;
mod logging;
mod quadlet;
mod secrets;
mod selfupdate;
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
    // --git-askpass is never invoked by a human: it's what `GIT_ASKPASS`
    // points `git` at (via a small wrapper script, see
    // `quadlet::gitsync::git::GitAuth::setup`) when a GitHub token is
    // configured, so `git` gets credentials for a private
    // `https://github.com/...` remote without the token ever appearing on a
    // command line or in a checkout's `.git/config`. Also runs synchronously
    // and exits immediately, same as `--hash-password`.
    if std::env::args().nth(1).as_deref() == Some("--git-askpass") {
        return git_askpass_cli();
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

/// Answers one `git` credential prompt (its text is `argv[2]`) with the
/// token from `SOOTH_GIT_ASKPASS_TOKEN` -- set by `git.rs` only on the `git`
/// child process it spawns, so this only ever sees it when actually invoked
/// as that process's askpass helper. A username prompt gets a placeholder
/// (`x-access-token`, the conventional non-empty username GitHub's own
/// token-auth docs use); anything else -- the password prompt -- gets the
/// token itself.
fn git_askpass_cli() -> anyhow::Result<()> {
    let prompt = std::env::args().nth(2).unwrap_or_default();
    if prompt.to_ascii_lowercase().starts_with("username") {
        println!("x-access-token");
    } else {
        println!(
            "{}",
            std::env::var("SOOTH_GIT_ASKPASS_TOKEN").unwrap_or_default()
        );
    }
    Ok(())
}

async fn run() -> anyhow::Result<()> {
    // Captured once, here, before anything else runs -- see the Gotchas note
    // in `AGENTS.md` on why `reexec` must reuse this exact value rather than
    // calling `current_exe()` again later. Once `selfupdate` has renamed a
    // freshly downloaded binary over this path, `current_exe()` (which reads
    // `/proc/self/exe`) resolves to `"<path> (deleted)"` for this
    // still-running process -- a string naming no real file.
    let exe_path = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("failed to resolve the running executable's path: {e}"))?;

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
    // Created here (rather than just before `AppState`, as previously) so it
    // already exists when `self_update` is constructed below.
    let restart = Arc::new(Notify::new());

    // Git-synced groups: one poll task per configured entry, kept live
    // (add/remove don't need a restart) rather than just a config snapshot.
    // Deliberately doesn't touch `systemd_client`/`events_tx` for reload --
    // its writes into the quadlet tree are picked up by the fs-watch task
    // below exactly like an external edit.
    let git_sync = quadlet::gitsync::GitSyncManager::new(
        Arc::new(config_path.clone()),
        events_tx.clone(),
        &exe_path,
        &config.github_token,
    );
    git_sync.start_all(&config.git_syncs, Arc::from(quadlet_dir.as_path()));

    // Self-update: one poll task, live-reconfigurable exactly like
    // `git_sync` above. A successful apply replaces the binary at `exe_path`
    // on disk and then notifies `restart` -- the same restart/re-exec
    // plumbing the Settings page's "Restart" button uses.
    let self_update = selfupdate::SelfUpdateManager::new(
        Arc::new(config_path.clone()),
        events_tx.clone(),
        restart.clone(),
    );
    self_update.start(&config.self_update);

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

    let state = AppState {
        config: Arc::new(config.clone()),
        quadlet_dir: Arc::new(quadlet_dir),
        systemd: Arc::new(systemd_client),
        health,
        config_path: Arc::new(config_path),
        events: events_tx,
        shutdown: shutdown_rx,
        restart: restart.clone(),
        git_sync: git_sync.clone(),
        self_update: self_update.clone(),
    };

    let app = web::build_router(state);
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    info!(addr = %config.bind_addr, url = %format!("http://{}", config.bind_addr), "sooth listening");

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
    git_sync.abort_all();
    self_update.abort();

    if restart_requested.load(Ordering::SeqCst) {
        // The listener is dropped by now, so the port is free for the fresh
        // process to rebind. `exec` only returns on failure.
        return reexec(&exe_path);
    }

    info!("shut down cleanly");
    Ok(())
}

/// Replace the current process with a fresh `sooth` at `exe`, inheriting argv
/// and the environment. This is how the in-app "Restart" button reloads
/// config, and how `selfupdate` applies an update: it works the same whether
/// sooth runs under systemd, the dev script, or a bare shell, none of which a
/// `systemctl restart` or a plain exit would.
///
/// `exe` must be the path captured by `run` at startup, not a fresh
/// `std::env::current_exe()` call here -- see the Gotchas note in
/// `AGENTS.md`. If `selfupdate` has replaced the binary at that path, this
/// process is still running from the now-unlinked original file, and
/// re-deriving the path at this point would resolve to `"<path> (deleted)"`
/// instead of the fresh binary actually sitting at `exe`.
#[cfg(unix)]
fn reexec(exe: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    info!(exe = %exe.display(), "restarting: re-executing");
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .exec();
    Err(anyhow::anyhow!(
        "failed to re-exec {}: {err}",
        exe.display()
    ))
}

#[cfg(not(unix))]
fn reexec(_exe: &std::path::Path) -> anyhow::Result<()> {
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
