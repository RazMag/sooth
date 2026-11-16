// Backstops "no panics in request-handling code" at compile time. Scoped
// off during test builds (`cfg(test)`), where `.unwrap()`/`.expect()` on a
// known-good fixture is normal and clearer than threading `Result` through
// every assertion.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod activity;
mod auth;
mod config;
mod containerhealth;
mod error;
mod events;
mod fleet;
mod health;
mod hostenv;
mod imageinfo;
mod imagetags;
mod journal;
mod link;
mod logging;
mod machine;
mod quadlet;
mod registries;
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

    let arg = std::env::args().nth(1);
    if arg.as_deref() == Some("--pair-code") {
        return pair_code_cli();
    }
    if arg.as_deref() == Some("--fingerprint") {
        return fingerprint_cli();
    }

    let rt = tokio::runtime::Runtime::new()?;
    match arg.as_deref() {
        Some("--agent") => rt.block_on(run_agent()),
        Some("--link-add") => rt.block_on(link_add_cli()),
        _ => rt.block_on(run()),
    }
}

fn config_path() -> std::path::PathBuf {
    std::env::var_os("SOOTH_CONFIG")
        .map(Into::into)
        .unwrap_or_else(config::default_config_path)
}

/// `sooth --pair-code [--read-only]`: makes a new pairing code for this
/// machine's agent (replacing any earlier one) and prints it with this
/// machine's fingerprint, for the hub's Add machine form.
fn pair_code_cli() -> anyhow::Result<()> {
    let read_only = std::env::args().any(|a| a == "--read-only");
    let dir = link::dir_beside(&config_path());
    let identity = link::Identity::load_or_create(&dir)?;
    let permission = if read_only {
        link::trust::Permission::ReadOnly
    } else {
        link::trust::Permission::Full
    };
    let code = link::trust::issue_code(&dir, permission)?;
    print_pairing(&code, &identity.fingerprint, permission);
    Ok(())
}

fn print_pairing(code: &str, fingerprint: &link::Fingerprint, permission: link::trust::Permission) {
    println!("Pairing code:  {code}");
    println!(
        "Valid for:     {} minutes, once",
        link::trust::CODE_LIFETIME_SECS / 60
    );
    println!("Access:        {}", permission.label());
    println!("Fingerprint:   {fingerprint}");
    println!();
    println!("In the hub, open Machines > Add machine, enter this machine's address");
    println!("and the code, and check that the fingerprint it shows matches the one above.");
}

/// `sooth --fingerprint`: this machine's link key fingerprint.
fn fingerprint_cli() -> anyhow::Result<()> {
    let identity = link::Identity::load_or_create(&link::dir_beside(&config_path()))?;
    println!("{}", identity.fingerprint);
    Ok(())
}

/// `sooth --link-add <name> <address> <code> <fingerprint>`: pairs this hub
/// with an agent without the browser, for scripts (`just dev-fleet`). The
/// fingerprint is required, since there's no one to confirm it.
async fn link_add_cli() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let [name, address, code, fingerprint] = args.as_slice() else {
        anyhow::bail!("usage: sooth --link-add <name> <https://address:port> <code> <fingerprint>");
    };
    let fingerprint = link::Fingerprint::parse(fingerprint)
        .ok_or_else(|| anyhow::anyhow!("'{fingerprint}' is not a fingerprint"))?;
    let hub_name = web::templates::overview::MachineInfo::detect().hostname;
    let fleet = fleet::Fleet::load(&config_path(), &hub_name)?;
    let (seen, hello) = fleet
        .probe(address)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if seen != fingerprint {
        anyhow::bail!("{address} shows key {seen}, not {fingerprint}");
    }
    let entry = fleet
        .add(address, fingerprint, code, name)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "Paired with {} ({}) as '{}', access {}.",
        hello.hostname,
        entry.address,
        entry.name,
        entry.permission.label()
    );
    Ok(())
}

/// A dev-only way to look at a linked machine before the fleet pages merge
/// machines: with `SOOTH_DEV_VIEW_MACHINE=<id>`, the pages read units,
/// status and logs (and run start/stop/restart) through that machine's
/// agent instead of this host. Detail pages, edits and the live log follow
/// still use this host's quadlet directory, so the agent must serve the same
/// one (`just dev-fleet` sets that up).
fn dev_view(fleet: &fleet::Fleet) -> anyhow::Result<Option<machine::Machine>> {
    let Ok(id) = std::env::var("SOOTH_DEV_VIEW_MACHINE") else {
        return Ok(None);
    };
    let linked = fleet
        .get(&id)
        .ok_or_else(|| anyhow::anyhow!("SOOTH_DEV_VIEW_MACHINE: no machine '{id}'"))?;
    tracing::warn!(machine = %linked.entry.name, "dev: pages read units through this machine's agent");
    Ok(Some(machine::Machine::Remote(linked.remote)))
}

/// `sooth --agent`: serves this host's machine API (`machine::api`) for a
/// hub to call, and nothing else -- no UI, no sessions, no background tasks.
/// Everything it answers is read fresh from disk and systemd per request.
async fn run_agent() -> anyhow::Result<()> {
    let config_path = config_path();
    let config = Config::load(Some(config_path.clone()))?;
    logging::init(config.log_filter.as_deref());

    let link_dir = link::dir_beside(&config_path);
    let identity = link::Identity::load_or_create(&link_dir)?;
    let hubs = Arc::new(link::trust::HubStore::load(&link_dir)?);

    let quadlet_dir = config.resolved_quadlet_dir()?;
    quadlet::discovery::ensure_dir(&quadlet_dir)?;
    info!(path = %quadlet_dir.display(), "using quadlet directory");
    let systemd_client = systemd::Client::connect().await?;
    let health = health::HealthCell::new(health::Health::check());

    let machine = machine::Machine::Local(machine::local::LocalMachine::new(
        Arc::new(quadlet_dir),
        Arc::new(systemd_client),
        health,
    ));
    let app = machine::api::router(machine::api::ApiState {
        machine,
        hubs: hubs.clone(),
    });

    let addr = config.agent_bind_addr;
    let tcp = tokio::net::TcpListener::bind(addr).await?;
    let listener = link::listener::TlsListener::new(tcp, identity.server_config()?)?;
    info!(%addr, url = %format!("https://{addr}"), fingerprint = %identity.fingerprint, "sooth agent listening");
    if hubs.hubs().is_empty() {
        let code = hubs.issue_code(link::trust::Permission::Full)?;
        info!(
            code = %code,
            "no hub is paired yet: add this machine in the hub with this code (valid 10 minutes; \
             `sooth --pair-code` makes a new one)"
        );
    } else {
        for hub in hubs.hubs() {
            info!(hub = %hub.name, access = hub.permission.label(), "paired hub");
        }
    }

    let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<link::listener::Peer>(),
    )
    .with_graceful_shutdown(shutdown_signal(
        shutdown_tx,
        Arc::new(Notify::new()),
        Arc::new(AtomicBool::new(false)),
    ))
    .await?;
    info!("agent shut down cleanly");
    Ok(())
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
    // Its writes into the quadlet tree are picked up by the fs-watch task
    // below exactly like an external edit; `systemd_client` is only for
    // restarting the running workloads an update changed.
    let git_sync = quadlet::gitsync::GitSyncManager::new(
        Arc::new(config_path.clone()),
        events_tx.clone(),
        Some(Arc::new(systemd_client.clone())),
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

    // Pinned-image tag check: one poll task, live-reconfigurable like
    // `self_update` above. Its tag listings live in memory only.
    let image_tags = imagetags::ImageTagsManager::new(
        Arc::new(config_path.clone()),
        Arc::new(quadlet_dir.clone()),
        events_tx.clone(),
    );
    image_tags.start(&config.image_tags);

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

    let activity_log = activity::ActivityLog::beside_config(&config_path);
    let quadlet_dir = Arc::new(quadlet_dir);
    let systemd_client = Arc::new(systemd_client);
    let fleet = fleet::Fleet::load(
        &config_path,
        &web::templates::overview::MachineInfo::detect().hostname,
    )?;
    info!(fingerprint = %fleet.fingerprint(), machines = fleet.machines().len(), "hub link key");
    let machine = match dev_view(&fleet)? {
        Some(remote) => remote,
        None => machine::Machine::Local(machine::local::LocalMachine::new(
            quadlet_dir.clone(),
            systemd_client.clone(),
            health.clone(),
        )),
    };
    let state = AppState {
        config: Arc::new(config.clone()),
        quadlet_dir,
        systemd: systemd_client,
        health,
        config_path: Arc::new(config_path),
        events: events_tx,
        shutdown: shutdown_rx,
        restart: restart.clone(),
        git_sync: git_sync.clone(),
        self_update: self_update.clone(),
        image_tags: image_tags.clone(),
        activity: activity_log,
        machine,
        fleet,
    };
    let activity_task = activity::spawn(state.clone());

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
    activity_task.abort();
    git_sync.abort_all();
    self_update.abort();
    image_tags.abort();

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
