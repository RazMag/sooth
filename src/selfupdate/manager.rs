//! [`SelfUpdateManager`]: the runtime supervisor for sooth's self-update.
//! Unlike `gitsync::GitSyncManager` (a map of per-group poll tasks) there is
//! only ever one target -- sooth itself -- so this is a single background
//! poll task, woken early by "Check now"/"Update now" instead of waiting out
//! its interval. All GitHub-API listing, checksum verification, download,
//! and the actual binary swap are delegated to the `self_update` crate (which
//! uses `self-replace` internally for the swap); this module only decides
//! *when* to check and *whether* to apply what it finds.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tracing::{info, warn};

use crate::events::{DashboardEvent, EventSender};

use super::{SelfUpdateConfig, SelfUpdateError, UpdateMode, UpdateState, UpdateStatus};

/// Shares `HealthCell`'s "`Clone`, `Arc`-backed" shape, just with more than
/// one field to share -- every clone of `AppState` sees the same live poll
/// task, config, and status.
#[derive(Clone)]
pub struct SelfUpdateManager {
    config: Arc<RwLock<SelfUpdateConfig>>,
    status: Arc<RwLock<UpdateStatus>>,
    /// Wakes the poll loop immediately instead of waiting out its interval --
    /// the "Check now" button.
    wake: Arc<Notify>,
    abort: Arc<Mutex<Option<AbortHandle>>>,
    config_path: Arc<PathBuf>,
    events: EventSender,
    /// Notified after a successful apply, reusing the exact plumbing the
    /// Settings page's "Restart" button uses (`AppState::restart` ->
    /// `main::shutdown_signal` -> `main::reexec`). Applying an update only
    /// ever replaces the binary on disk; restarting into it is not this
    /// module's job.
    restart: Arc<Notify>,
    /// Test-only overrides for `self_update`'s GitHub API base URL and
    /// install path, both otherwise left at their real defaults (the public
    /// GitHub API, and the current executable's own path). Always `None`
    /// outside `#[cfg(test)]` code, which is the only place that ever calls
    /// [`with_api_base_for_test`](Self::with_api_base_for_test) /
    /// [`with_install_path_for_test`](Self::with_install_path_for_test) --
    /// without them, exercising `update_async` in a test would try to
    /// download from the real GitHub API and overwrite the test binary
    /// itself.
    test_api_base: Option<String>,
    test_install_path: Option<PathBuf>,
}

impl SelfUpdateManager {
    pub fn new(config_path: Arc<PathBuf>, events: EventSender, restart: Arc<Notify>) -> Self {
        Self {
            config: Arc::new(RwLock::new(SelfUpdateConfig::default())),
            status: Arc::new(RwLock::new(UpdateStatus::default())),
            wake: Arc::new(Notify::new()),
            abort: Arc::new(Mutex::new(None)),
            config_path,
            events,
            restart,
            test_api_base: None,
            test_install_path: None,
        }
    }

    /// Spawns (or respawns) the poll task with `config` -- called once from
    /// `main` at startup, and again by `configure` whenever settings change,
    /// live, no process restart needed.
    pub fn start(&self, config: &SelfUpdateConfig) {
        self.spawn(config.clone());
    }

    fn spawn(&self, config: SelfUpdateConfig) {
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = config;
        let mgr = self.clone();
        let handle = tokio::spawn(async move { mgr.run_loop().await });
        let prev = self
            .abort
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(handle.abort_handle());
        if let Some(prev) = prev {
            prev.abort();
        }
    }

    /// Persists new settings and restarts the poll task with them.
    pub fn configure(&self, config: SelfUpdateConfig) -> Result<(), SelfUpdateError> {
        crate::config::patch_self_update(&self.config_path, |c| *c = config.clone())?;
        info!(
            mode = config.mode.as_str(),
            repo = %config.repo,
            "self-update settings changed"
        );
        self.spawn(config);
        let _ = self.events.send(DashboardEvent::SelfUpdateChanged);
        Ok(())
    }

    /// "Check now": wakes the poll loop immediately instead of waiting out
    /// its interval.
    pub fn check_now(&self) {
        self.wake.notify_one();
    }

    /// "Download update": the explicit action for `Notify` mode, once an
    /// update has been found. Downloads, verifies, and swaps the binary onto
    /// disk exactly like `Auto` mode would, but stops at
    /// `UpdateState::ReadyToRestart` instead of notifying `restart` itself --
    /// that's the separate "Install and restart" action (the Settings page's
    /// existing Restart button). Errors if the last check didn't record an
    /// available update, or an attempt is already in flight.
    pub async fn download_now(&self) -> Result<(), SelfUpdateError> {
        let config = self.config_snapshot();
        if config.mode == UpdateMode::Off {
            return Err(SelfUpdateError::Disabled);
        }
        {
            let status = self.status.read().unwrap_or_else(|e| e.into_inner());
            match &status.state {
                UpdateState::UpdateAvailable { .. } => {}
                UpdateState::Checking | UpdateState::Downloading => {
                    return Err(SelfUpdateError::Busy);
                }
                _ => return Err(SelfUpdateError::NoUpdateAvailable),
            }
        }
        self.attempt(&config, true).await
    }

    /// The current status, for rendering the Settings page's Updates card.
    pub fn snapshot(&self) -> UpdateStatus {
        self.status
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn config_snapshot(&self) -> SelfUpdateConfig {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Aborts the poll task -- called from `main` at shutdown alongside
    /// `git_sync.abort_all()`; like that task, `run_loop` never exits on its
    /// own.
    pub fn abort(&self) {
        if let Some(h) = self.abort.lock().unwrap_or_else(|e| e.into_inner()).take() {
            h.abort();
        }
    }

    fn set_state(&self, state: UpdateState) {
        let mut g = self.status.write().unwrap_or_else(|e| e.into_inner());
        g.state = state;
        g.checked_at = Some(SystemTime::now());
    }

    async fn run_loop(self) {
        loop {
            let config = self.config_snapshot();
            if config.mode != UpdateMode::Off {
                let _ = self.attempt(&config, false).await;
            }
            let interval = Duration::from_secs(config.poll_interval_secs.max(60));
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = self.wake.notified() => {}
            }
        }
    }

    /// One check-and-maybe-download attempt. Never panics; on any failure
    /// logs and sets `UpdateState::Error`, same "log and retry next
    /// interval" posture as `gitsync::manager::attempt`. `force_download` is
    /// set by `download_now` (downloads regardless of `mode`); the periodic
    /// loop passes `false` and only downloads automatically when
    /// `mode == Auto`. Either way, only `Auto` mode notifies `restart`
    /// itself once downloaded -- a manual download (`Notify` mode) always
    /// lands on `ReadyToRestart` and waits for the separate "Install and
    /// restart" action.
    async fn attempt(
        &self,
        config: &SelfUpdateConfig,
        force_download: bool,
    ) -> Result<(), SelfUpdateError> {
        self.set_state(UpdateState::Checking);
        let _ = self.events.send(DashboardEvent::SelfUpdateChanged);

        let updater = match self.build_updater(config) {
            Ok(u) => u,
            Err(e) => return self.report_error(e),
        };

        let newer = match updater.is_update_available_async().await {
            Ok(r) => r,
            Err(e) => return self.report_error(e.into()),
        };
        let Some(release) = newer else {
            self.set_state(UpdateState::UpToDate);
            let _ = self.events.send(DashboardEvent::SelfUpdateChanged);
            return Ok(());
        };

        if !(config.mode == UpdateMode::Auto || force_download) {
            self.set_state(UpdateState::UpdateAvailable {
                version: release.version().to_string(),
            });
            let _ = self.events.send(DashboardEvent::SelfUpdateChanged);
            return Ok(());
        }

        self.set_state(UpdateState::Downloading);
        let _ = self.events.send(DashboardEvent::SelfUpdateChanged);

        let status = match updater.update_async().await {
            Ok(s) => s,
            Err(e) => return self.report_error(e.into()),
        };

        if !status.is_updated() {
            // Raced: something else (another download, or a manual replace)
            // already updated between the check above and this attempt.
            self.set_state(UpdateState::UpToDate);
            let _ = self.events.send(DashboardEvent::SelfUpdateChanged);
            return Ok(());
        }

        if config.mode == UpdateMode::Auto {
            self.set_state(UpdateState::Applying);
            let _ = self.events.send(DashboardEvent::SelfUpdateChanged);
            info!(
                version = status.version(),
                "self-update downloaded; requesting restart"
            );
            self.restart.notify_one();
        } else {
            info!(
                version = status.version(),
                "self-update downloaded; ready to install"
            );
            self.set_state(UpdateState::ReadyToRestart {
                version: status.version().to_string(),
            });
            let _ = self.events.send(DashboardEvent::SelfUpdateChanged);
        }
        Ok(())
    }

    fn report_error(&self, error: SelfUpdateError) -> Result<(), SelfUpdateError> {
        warn!(error = %error, "self-update attempt failed");
        self.set_state(UpdateState::Error(error.to_string()));
        let _ = self.events.send(DashboardEvent::SelfUpdateChanged);
        Err(error)
    }

    /// Builds a `self_update` GitHub updater from `config`. `bin_install_path`
    /// and `target` are left at their defaults (the current executable's
    /// path and this build's own compiled target triple, respectively) --
    /// exactly right in production, since sooth only ever updates itself.
    /// `unattended()` disables the interactive confirmation prompt and
    /// status output `self_update` otherwise prints to stdout, both wrong
    /// for a background task with no terminal attached. Checksum
    /// verification against GitHub's own published per-asset digest is on
    /// by default with the `checksums` feature, so no extra configuration
    /// is needed for that either.
    fn build_updater(
        &self,
        config: &SelfUpdateConfig,
    ) -> Result<self_update::backends::github::AsyncUpdate, SelfUpdateError> {
        let (owner, name) = super::split_repo(&config.repo)?;
        let mut builder = self_update::backends::github::Update::configure();
        builder
            .repo_owner(&owner)
            .repo_name(&name)
            .bin_name("sooth")
            .current_version(env!("CARGO_PKG_VERSION"))
            .update_strategy(self_update::UpdateStrategy::Latest)
            .unattended();
        if let Some(base) = &self.test_api_base {
            builder.api_base_url(base);
        }
        if let Some(path) = &self.test_install_path {
            builder.bin_install_path(path);
        }
        builder.build_async().map_err(SelfUpdateError::from)
    }
}

#[cfg(test)]
impl SelfUpdateManager {
    /// Points `self_update`'s GitHub client at a local mock server instead
    /// of the real API.
    fn with_api_base_for_test(mut self, base: impl Into<String>) -> Self {
        self.test_api_base = Some(base.into());
        self
    }

    /// Points the binary swap at a throwaway file instead of the real
    /// `current_exe()` -- without this, exercising `update_async` would try
    /// to overwrite the test binary itself.
    fn with_install_path_for_test(mut self, path: impl Into<PathBuf>) -> Self {
        self.test_install_path = Some(path.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc as StdArc;

    use axum::Router;
    use axum::extract::State;
    use axum::routing::get;
    use sha2::{Digest, Sha256};

    use super::*;

    fn manager() -> (SelfUpdateManager, tempfile::TempDir) {
        let config_dir = tempfile::tempdir().unwrap();
        let config_path = Arc::new(config_dir.path().join("config.toml"));
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let restart = Arc::new(Notify::new());
        (SelfUpdateManager::new(config_path, tx, restart), config_dir)
    }

    fn sha256_hex(data: &[u8]) -> String {
        let digest = Sha256::digest(data);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    struct MockRelease {
        asset_name: String,
        /// `sha256:<hex>` as github's own per-asset `digest` field would
        /// read -- deliberately a *separate* input from the bytes actually
        /// served, so `mock_github_with_digest` can advertise a digest that
        /// doesn't match (see `checksum_mismatch_...`).
        digest: String,
        binary: Vec<u8>,
        base: String,
    }

    async fn releases_handler(State(state): State<StdArc<MockRelease>>) -> String {
        format!(
            r#"[{{"tag_name":"v9.9.9","created_at":"2024-01-01T00:00:00Z","name":"v9.9.9","assets":[{{"name":"{name}","url":"{base}/assets/{name}","digest":"{digest}"}}]}}]"#,
            name = state.asset_name,
            base = state.base,
            digest = state.digest,
        )
    }

    async fn asset_handler(State(state): State<StdArc<MockRelease>>) -> Vec<u8> {
        state.binary.clone()
    }

    /// A tiny local stand-in for the two GitHub API endpoints `self_update`
    /// calls: the release listing (`/repos/{owner}/{repo}/releases`) and the
    /// asset download it points at. Serves one release, `v9.9.9`, with one
    /// asset matching this build's own compiled target triple -- the same
    /// default `build_updater` leaves `self_update` to resolve on its own,
    /// so no `.target(..)` override is needed on either side. Returns the
    /// base URL to pass to `with_api_base_for_test`.
    async fn mock_github_with_digest(binary: Vec<u8>, digest_hex: String) -> String {
        let asset_name = format!("sooth-{}", self_update::get_target());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = StdArc::new(MockRelease {
            asset_name,
            digest: format!("sha256:{digest_hex}"),
            binary,
            base: base.clone(),
        });
        let app = Router::new()
            .route("/repos/{owner}/{repo}/releases", get(releases_handler))
            .route("/assets/{name}", get(asset_handler))
            .with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        base
    }

    async fn mock_github(binary: Vec<u8>) -> String {
        let digest_hex = sha256_hex(&binary);
        mock_github_with_digest(binary, digest_hex).await
    }

    fn ready_config(repo: &str) -> SelfUpdateConfig {
        SelfUpdateConfig {
            mode: UpdateMode::Auto,
            poll_interval_secs: 3600,
            repo: repo.to_string(),
        }
    }

    #[tokio::test]
    async fn auto_mode_applies_and_notifies_restart() {
        let (mgr, _config_dir) = manager();
        let base = mock_github(b"new sooth binary".to_vec()).await;
        let exe_dir = tempfile::tempdir().unwrap();
        let exe_path = exe_dir.path().join("sooth");
        std::fs::write(&exe_path, b"old sooth binary").unwrap();

        let mgr = mgr
            .with_api_base_for_test(base)
            .with_install_path_for_test(exe_path.clone());
        let restart = mgr.restart.clone();

        mgr.configure(ready_config("someone/sooth")).unwrap();

        tokio::time::timeout(Duration::from_secs(5), restart.notified())
            .await
            .expect("auto mode must notify restart after applying");
        assert_eq!(std::fs::read(&exe_path).unwrap(), b"new sooth binary");
    }

    #[tokio::test]
    async fn off_mode_never_checks() {
        let (mgr, _config_dir) = manager();
        let base = mock_github(b"new sooth binary".to_vec()).await;
        let mgr = mgr.with_api_base_for_test(base);
        mgr.configure(SelfUpdateConfig {
            mode: UpdateMode::Off,
            ..ready_config("someone/sooth")
        })
        .unwrap();

        // Give the loop a few iterations' worth of time to (not) run.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(mgr.snapshot().state, UpdateState::Idle);
    }

    #[tokio::test]
    async fn notify_mode_flags_availability_without_downloading() {
        let (mgr, _config_dir) = manager();
        let base = mock_github(b"new sooth binary".to_vec()).await;
        let exe_dir = tempfile::tempdir().unwrap();
        let exe_path = exe_dir.path().join("sooth");
        std::fs::write(&exe_path, b"old sooth binary").unwrap();

        let mgr = mgr
            .with_api_base_for_test(base)
            .with_install_path_for_test(exe_path.clone());
        mgr.configure(SelfUpdateConfig {
            mode: UpdateMode::Notify,
            ..ready_config("someone/sooth")
        })
        .unwrap();

        for _ in 0..100 {
            if matches!(mgr.snapshot().state, UpdateState::UpdateAvailable { .. }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            matches!(mgr.snapshot().state, UpdateState::UpdateAvailable { version } if version == "9.9.9"),
            "expected UpdateAvailable(9.9.9), got {:?}",
            mgr.snapshot()
        );
        assert_eq!(
            std::fs::read(&exe_path).unwrap(),
            b"old sooth binary",
            "notify mode must never touch the binary on its own"
        );
    }

    #[tokio::test]
    async fn download_now_downloads_but_does_not_restart() {
        let (mgr, _config_dir) = manager();
        let base = mock_github(b"new sooth binary".to_vec()).await;
        let exe_dir = tempfile::tempdir().unwrap();
        let exe_path = exe_dir.path().join("sooth");
        std::fs::write(&exe_path, b"old sooth binary").unwrap();

        let mgr = mgr
            .with_api_base_for_test(base)
            .with_install_path_for_test(exe_path.clone());
        let restart = mgr.restart.clone();
        mgr.configure(SelfUpdateConfig {
            mode: UpdateMode::Notify,
            ..ready_config("someone/sooth")
        })
        .unwrap();

        for _ in 0..100 {
            if matches!(mgr.snapshot().state, UpdateState::UpdateAvailable { .. }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        mgr.download_now().await.unwrap();

        assert!(
            matches!(mgr.snapshot().state, UpdateState::ReadyToRestart { version } if version == "9.9.9"),
            "expected ReadyToRestart(9.9.9), got {:?}",
            mgr.snapshot()
        );
        assert_eq!(
            std::fs::read(&exe_path).unwrap(),
            b"new sooth binary",
            "download_now must swap the binary onto disk"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(200), restart.notified())
                .await
                .is_err(),
            "a manual download in Notify mode must not restart on its own -- \
             that's the separate Install-and-restart action"
        );
    }

    #[tokio::test]
    async fn download_now_errors_without_a_pending_update() {
        let (mgr, _config_dir) = manager();
        mgr.configure(ready_config("someone/sooth")).unwrap();
        // No poll has run yet -- status is still `Idle`.
        assert!(matches!(
            mgr.download_now().await,
            Err(SelfUpdateError::NoUpdateAvailable)
        ));
    }

    #[tokio::test]
    async fn download_now_errors_while_off() {
        let (mgr, _config_dir) = manager();
        mgr.configure(SelfUpdateConfig {
            mode: UpdateMode::Off,
            ..ready_config("someone/sooth")
        })
        .unwrap();
        assert!(matches!(
            mgr.download_now().await,
            Err(SelfUpdateError::Disabled)
        ));
    }

    #[tokio::test]
    async fn checksum_mismatch_leaves_the_binary_untouched() {
        let (mgr, _config_dir) = manager();
        // Advertise a digest that doesn't match the bytes actually served --
        // the same shape of failure as a tampered-in-transit download.
        let wrong_digest = sha256_hex(b"not the served bytes");
        let base = mock_github_with_digest(b"new sooth binary".to_vec(), wrong_digest).await;
        let exe_dir = tempfile::tempdir().unwrap();
        let exe_path = exe_dir.path().join("sooth");
        std::fs::write(&exe_path, b"old sooth binary").unwrap();

        let mgr = mgr
            .with_api_base_for_test(base)
            .with_install_path_for_test(exe_path.clone());
        let restart = mgr.restart.clone();
        mgr.configure(ready_config("someone/sooth")).unwrap();

        for _ in 0..100 {
            if matches!(mgr.snapshot().state, UpdateState::Error(_)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            matches!(mgr.snapshot().state, UpdateState::Error(_)),
            "expected Error, got {:?}",
            mgr.snapshot()
        );
        assert_eq!(
            std::fs::read(&exe_path).unwrap(),
            b"old sooth binary",
            "a checksum mismatch must never touch the installed binary"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(200), restart.notified())
                .await
                .is_err(),
            "a failed apply must never notify restart"
        );
    }

    #[tokio::test]
    async fn configure_persists_and_respawns() {
        let (mgr, config_dir) = manager();
        mgr.configure(ready_config("someone/sooth")).unwrap();
        mgr.configure(SelfUpdateConfig {
            poll_interval_secs: 120,
            ..ready_config("someone/sooth")
        })
        .unwrap();

        assert_eq!(mgr.config_snapshot().poll_interval_secs, 120);

        // Persisted too, not just the in-memory entry.
        let reloaded =
            crate::config::Config::load(Some(config_dir.path().join("config.toml"))).unwrap();
        assert_eq!(reloaded.self_update.poll_interval_secs, 120);
        assert_eq!(reloaded.self_update.repo, "someone/sooth");
    }
}
