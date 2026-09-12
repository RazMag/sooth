//! [`GitSyncManager`]: the runtime supervisor for every configured
//! git-synced group. One background task per entry (`run_loop`) polls its
//! remote on its own interval and is woken immediately by `sync_now` /
//! `force_resync`; `add`/`remove` change the running set live, no restart
//! needed, unlike the rest of `Config`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tracing::{info, warn};

use crate::events::{DashboardEvent, EventSender};
use crate::quadlet::naming;

use super::{GitSyncConfig, GitSyncError, SyncState, SyncStatus, git};

struct SyncEntry {
    config: GitSyncConfig,
    status: Arc<RwLock<SyncStatus>>,
    /// Wakes the entry's `run_loop` immediately instead of waiting out its
    /// poll interval -- the "Sync now" button.
    wake: Arc<Notify>,
    abort: AbortHandle,
}

/// Shares `HealthCell`'s shape -- `Clone`, `Arc`-backed, so every `AppState`
/// clone sees the same live data -- but keyed per group rather than a single
/// snapshot, since each sync has its own independent status and lifecycle.
#[derive(Clone)]
pub struct GitSyncManager {
    entries: Arc<RwLock<HashMap<String, SyncEntry>>>,
    config_path: Arc<PathBuf>,
    events: EventSender,
}

impl GitSyncManager {
    pub fn new(config_path: Arc<PathBuf>, events: EventSender) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            config_path,
            events,
        }
    }

    /// Spawns one poll task per already-configured entry -- called once from
    /// `main` at startup. Distinct from `add`, which also validates a
    /// brand-new group, clones it, and persists it.
    pub fn start_all(&self, configs: &[GitSyncConfig], quadlet_dir: Arc<Path>) {
        for config in configs {
            self.spawn(config.clone(), quadlet_dir.clone());
        }
    }

    fn spawn(&self, config: GitSyncConfig, quadlet_dir: Arc<Path>) {
        let group = config.group.clone();
        let status = Arc::new(RwLock::new(SyncStatus::default()));
        let wake = Arc::new(Notify::new());
        let handle = tokio::spawn(run_loop(
            config.clone(),
            quadlet_dir,
            status.clone(),
            wake.clone(),
            self.events.clone(),
        ));
        let entry = SyncEntry {
            config,
            status,
            wake,
            abort: handle.abort_handle(),
        };
        let prev = self
            .entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(group, entry);
        if let Some(prev) = prev {
            prev.abort.abort();
        }
    }

    /// Validates and clones a new sync, persists it, then starts polling it
    /// immediately.
    pub async fn add(
        &self,
        group: String,
        remote: String,
        branch: Option<String>,
        poll_interval_secs: u64,
        quadlet_dir: &Path,
    ) -> Result<(), GitSyncError> {
        if group.is_empty() || !naming::valid_group(&group) {
            return Err(GitSyncError::Validation(format!("invalid group '{group}'")));
        }
        if self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&group)
        {
            return Err(GitSyncError::AlreadyExists(group));
        }
        let target = quadlet_dir.join(&group);
        if target.exists() && std::fs::read_dir(&target)?.next().is_some() {
            return Err(GitSyncError::Validation(format!(
                "'{group}' is not empty -- point a git-sync at a new or empty group"
            )));
        }
        std::fs::create_dir_all(&target)?;

        git::clone(&remote, branch.as_deref(), &target).await?;
        let resolved_branch = resolve_branch(&target, branch.as_deref()).await?;

        let config = GitSyncConfig {
            group: group.clone(),
            remote,
            branch: Some(resolved_branch),
            poll_interval_secs: poll_interval_secs.max(1),
        };
        crate::config::patch_git_syncs(&self.config_path, |list| list.push(config.clone()))
            .map_err(|e| GitSyncError::Failed(format!("failed to save config: {e}")))?;

        info!(group = %config.group, remote = %config.remote, "git-sync added");
        self.spawn(config, Arc::from(quadlet_dir));
        let _ = self.events.send(DashboardEvent::GitSyncChanged);
        Ok(())
    }

    /// Updates an existing sync's remote/branch/poll interval in place --
    /// the group itself (its identity and on-disk location) can't be
    /// changed here, only re-added. A blank `branch` means "leave the
    /// currently-tracked branch alone" (unlike `add`, where it means "track
    /// the remote's default" -- there is no "current" branch yet then).
    /// When the remote or the resolved branch actually changes, re-points
    /// `origin` and checks out the new branch (discarding the working
    /// tree's previous contents, same posture as `force_resync`) before
    /// persisting and restarting the poll task with the new config.
    pub async fn edit(
        &self,
        group: &str,
        remote: String,
        branch: Option<String>,
        poll_interval_secs: u64,
        quadlet_dir: &Path,
    ) -> Result<(), GitSyncError> {
        let existing = {
            let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
            entries
                .get(group)
                .ok_or_else(|| GitSyncError::NotFound(group.to_string()))?
                .config
                .clone()
        };
        let target = quadlet_dir.join(group);

        let remote_changed = remote != existing.remote;
        if remote_changed {
            git::set_remote_url(&target, &remote).await?;
        }
        let resolved_branch = branch
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| existing.branch.clone().unwrap_or_default());
        let branch_changed = Some(&resolved_branch) != existing.branch.as_ref();

        if remote_changed || branch_changed {
            git::fetch_ref(&target, &resolved_branch).await?;
            git::checkout_branch(&target, &resolved_branch).await?;
        }

        let config = GitSyncConfig {
            group: group.to_string(),
            remote,
            branch: Some(resolved_branch),
            poll_interval_secs: poll_interval_secs.max(1),
        };
        crate::config::patch_git_syncs(&self.config_path, |list| {
            if let Some(entry) = list.iter_mut().find(|c| c.group == group) {
                *entry = config.clone();
            }
        })
        .map_err(|e| GitSyncError::Failed(format!("failed to save config: {e}")))?;

        info!(group, "git-sync edited");
        self.spawn(config, Arc::from(quadlet_dir));
        let _ = self.events.send(DashboardEvent::GitSyncChanged);
        Ok(())
    }

    /// Wakes the entry's poll loop immediately rather than waiting for its
    /// interval -- the "Sync now" button. Still subject to the same
    /// fast-forward-only check as a routine poll; a diverged checkout needs
    /// `force_resync` instead.
    pub fn sync_now(&self, group: &str) -> Result<(), GitSyncError> {
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
        let entry = entries
            .get(group)
            .ok_or_else(|| GitSyncError::NotFound(group.to_string()))?;
        entry.wake.notify_one();
        Ok(())
    }

    /// Recovery for a diverged checkout: unconditionally resets to the
    /// remote, discarding local divergence. Kept distinct from `sync_now` so
    /// that discarding data is always an explicit, separately-named action,
    /// never an accidental side effect of the routine "check for updates"
    /// path.
    pub async fn force_resync(&self, group: &str, quadlet_dir: &Path) -> Result<(), GitSyncError> {
        let (config, status) = {
            let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
            let entry = entries
                .get(group)
                .ok_or_else(|| GitSyncError::NotFound(group.to_string()))?;
            (entry.config.clone(), entry.status.clone())
        };
        let target = quadlet_dir.join(&config.group);
        let branch = config.branch.as_deref().unwrap_or("HEAD");

        set_state(&status, SyncState::Syncing);
        let _ = self.events.send(DashboardEvent::GitSyncChanged);

        let result: Result<String, GitSyncError> = async {
            git::fetch(&target, branch).await?;
            let remote_head = git::rev_parse(&target, &format!("origin/{branch}")).await?;
            git::reset_hard(&target, &remote_head).await?;
            Ok(remote_head)
        }
        .await;

        match result {
            Ok(commit) => {
                info!(group = %config.group, %commit, "git-sync force-resynced");
                set_state(&status, SyncState::UpToDate { commit });
                let _ = self.events.send(DashboardEvent::GitSyncChanged);
                Ok(())
            }
            Err(e) => {
                set_state(&status, SyncState::Error(e.to_string()));
                let _ = self.events.send(DashboardEvent::GitSyncChanged);
                Err(e)
            }
        }
    }

    /// Stops tracking a sync: aborts its poll task, drops the config entry,
    /// and -- when `delete_files` is set -- removes the group directory too,
    /// reusing `quadlet::writer::delete_dir` (the same primitive an ordinary
    /// group delete uses).
    pub fn remove(
        &self,
        group: &str,
        quadlet_dir: &Path,
        delete_files: bool,
    ) -> Result<(), GitSyncError> {
        let entry = self
            .entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(group)
            .ok_or_else(|| GitSyncError::NotFound(group.to_string()))?;
        entry.abort.abort();

        crate::config::patch_git_syncs(&self.config_path, |list| list.retain(|c| c.group != group))
            .map_err(|e| GitSyncError::Failed(format!("failed to save config: {e}")))?;

        if delete_files {
            crate::quadlet::writer::delete_dir(quadlet_dir, group)
                .map_err(|e| GitSyncError::Failed(e.to_string()))?;
        }
        info!(group, delete_files, "git-sync removed");
        let _ = self.events.send(DashboardEvent::GitSyncChanged);
        Ok(())
    }

    /// A snapshot of every configured sync and its current status, sorted by
    /// group, for rendering the Git Sync page.
    pub fn snapshot(&self) -> Vec<(GitSyncConfig, SyncStatus)> {
        let mut out: Vec<_> = self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|e| {
                (
                    e.config.clone(),
                    e.status.read().unwrap_or_else(|e| e.into_inner()).clone(),
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.group.cmp(&b.0.group));
        out
    }

    /// Just the configured group paths, sorted -- the cheap query
    /// `web::core`'s move/create/rename guards need (they only care which
    /// paths are git-managed, not each one's remote/branch/status).
    pub fn synced_groups(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Aborts every poll task -- called from `main` at shutdown alongside
    /// the status-watch and fs-watch tasks. Like those, `run_loop` never
    /// exits on its own, so skipping this would block `Runtime::drop`
    /// forever despite having already logged a clean shutdown.
    pub fn abort_all(&self) {
        for entry in self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            entry.abort.abort();
        }
    }
}

fn set_state(status: &Arc<RwLock<SyncStatus>>, state: SyncState) {
    let mut guard = status.write().unwrap_or_else(|e| e.into_inner());
    guard.state = state;
    guard.checked_at = Some(SystemTime::now());
}

/// The branch a sync should track: the configured one, or -- when it was
/// left unset so `add` clones the remote's default branch -- whatever
/// actually got checked out.
async fn resolve_branch(target: &Path, configured: Option<&str>) -> Result<String, GitSyncError> {
    match configured {
        Some(b) => Ok(b.to_string()),
        None => git::current_branch(target).await,
    }
}

async fn run_loop(
    config: GitSyncConfig,
    quadlet_dir: Arc<Path>,
    status: Arc<RwLock<SyncStatus>>,
    wake: Arc<Notify>,
    events: EventSender,
) {
    loop {
        attempt(&config, &quadlet_dir, &status, &events).await;
        let interval = Duration::from_secs(config.poll_interval_secs.max(1));
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = wake.notified() => {}
        }
    }
}

/// One fetch-and-maybe-pull attempt: clones on first run, otherwise fetches
/// and fast-forwards the local checkout when the remote has moved. Never
/// panics or returns -- failures are logged and stored on `status` so the
/// loop just tries again next interval.
async fn attempt(
    config: &GitSyncConfig,
    quadlet_dir: &Path,
    status: &Arc<RwLock<SyncStatus>>,
    events: &EventSender,
) {
    let target = quadlet_dir.join(&config.group);
    let already_cloned = target.join(".git").is_dir();

    if !already_cloned {
        set_state(status, SyncState::Cloning);
        let _ = events.send(DashboardEvent::GitSyncChanged);
        if let Err(e) = std::fs::create_dir_all(&target) {
            report_error(&config.group, status, events, e.into());
            return;
        }
        if let Err(e) = git::clone(&config.remote, config.branch.as_deref(), &target).await {
            report_error(&config.group, status, events, e);
            return;
        }
    } else {
        set_state(status, SyncState::Checking);
        let _ = events.send(DashboardEvent::GitSyncChanged);
    }

    let branch = match resolve_branch(&target, config.branch.as_deref()).await {
        Ok(b) => b,
        Err(e) => {
            report_error(&config.group, status, events, e);
            return;
        }
    };

    if already_cloned && let Err(e) = git::fetch(&target, &branch).await {
        report_error(&config.group, status, events, e);
        return;
    }

    let local = match git::rev_parse(&target, "HEAD").await {
        Ok(rev) => rev,
        Err(e) => {
            report_error(&config.group, status, events, e);
            return;
        }
    };
    let remote = match git::rev_parse(&target, &format!("origin/{branch}")).await {
        Ok(rev) => rev,
        Err(e) => {
            report_error(&config.group, status, events, e);
            return;
        }
    };

    if local == remote {
        set_state(status, SyncState::UpToDate { commit: local });
        let _ = events.send(DashboardEvent::GitSyncChanged);
        return;
    }

    match git::is_ancestor(&target, &local, &remote).await {
        Ok(true) => {
            set_state(status, SyncState::Syncing);
            let _ = events.send(DashboardEvent::GitSyncChanged);
            if let Err(e) = git::reset_hard(&target, &remote).await {
                report_error(&config.group, status, events, e);
                return;
            }
            info!(group = %config.group, commit = %remote, "git-sync updated");
            set_state(status, SyncState::UpToDate { commit: remote });
            let _ = events.send(DashboardEvent::GitSyncChanged);
        }
        Ok(false) => report_error(
            &config.group,
            status,
            events,
            GitSyncError::Failed(
                "local checkout has diverged from the remote; use \"Force resync\" \
                 to discard the local divergence"
                    .to_string(),
            ),
        ),
        Err(e) => report_error(&config.group, status, events, e),
    }
}

fn report_error(
    group: &str,
    status: &Arc<RwLock<SyncStatus>>,
    events: &EventSender,
    error: GitSyncError,
) {
    warn!(group, error = %error, "git-sync attempt failed");
    set_state(status, SyncState::Error(error.to_string()));
    let _ = events.send(DashboardEvent::GitSyncChanged);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            // Override any host-global `commit.gpgsign=true` -- see the
            // identical comment in `gitsync::git`'s test helper.
            .arg("-c")
            .arg("commit.gpgsign=false")
            .args(args)
            .env("GIT_AUTHOR_NAME", "sooth-test")
            .env("GIT_AUTHOR_EMAIL", "sooth-test@example.invalid")
            .env("GIT_COMMITTER_NAME", "sooth-test")
            .env("GIT_COMMITTER_EMAIL", "sooth-test@example.invalid")
            .output()
            .expect("git must be installed to run this test")
    }

    fn make_origin() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            git(dir.path(), &["init", "--quiet", "--initial-branch=main"])
                .status
                .success()
        );
        std::fs::write(dir.path().join("web.container"), "[Container]\nImage=x\n").unwrap();
        assert!(git(dir.path(), &["add", "."]).status.success());
        assert!(
            git(dir.path(), &["commit", "--quiet", "-m", "initial"])
                .status
                .success()
        );
        dir
    }

    fn manager() -> (GitSyncManager, tempfile::TempDir) {
        let config_dir = tempfile::tempdir().unwrap();
        let config_path = Arc::new(config_dir.path().join("config.toml"));
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        (GitSyncManager::new(config_path, tx), config_dir)
    }

    #[tokio::test]
    async fn add_clones_and_reports_up_to_date() {
        let origin = make_origin();
        let quadlet_dir = tempfile::tempdir().unwrap();
        let (mgr, _config_dir) = manager();

        mgr.add(
            "synced".to_string(),
            origin.path().display().to_string(),
            Some("main".to_string()),
            1,
            quadlet_dir.path(),
        )
        .await
        .unwrap();

        assert!(quadlet_dir.path().join("synced/web.container").is_file());

        // The background loop's first attempt races this assertion; give it
        // a moment to land on `UpToDate` rather than still `Cloning`.
        for _ in 0..50 {
            let snap = mgr.snapshot();
            if matches!(snap[0].1.state, SyncState::UpToDate { .. }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("git-sync never reached UpToDate: {:?}", mgr.snapshot());
    }

    #[tokio::test]
    async fn add_rejects_a_nonempty_group() {
        let origin = make_origin();
        let quadlet_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(quadlet_dir.path().join("taken")).unwrap();
        std::fs::write(quadlet_dir.path().join("taken/x.container"), "").unwrap();
        let (mgr, _config_dir) = manager();

        let err = mgr
            .add(
                "taken".to_string(),
                origin.path().display().to_string(),
                Some("main".to_string()),
                60,
                quadlet_dir.path(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, GitSyncError::Validation(_)));
    }

    #[tokio::test]
    async fn remove_stops_the_task_and_optionally_deletes_files() {
        let origin = make_origin();
        let quadlet_dir = tempfile::tempdir().unwrap();
        let (mgr, _config_dir) = manager();
        mgr.add(
            "synced".to_string(),
            origin.path().display().to_string(),
            Some("main".to_string()),
            60,
            quadlet_dir.path(),
        )
        .await
        .unwrap();

        mgr.remove("synced", quadlet_dir.path(), true).unwrap();

        assert!(mgr.snapshot().is_empty());
        assert!(!quadlet_dir.path().join("synced").exists());
        assert!(matches!(
            mgr.sync_now("synced"),
            Err(GitSyncError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn edit_updates_poll_interval_without_touching_git() {
        let origin = make_origin();
        let quadlet_dir = tempfile::tempdir().unwrap();
        let (mgr, config_dir) = manager();
        mgr.add(
            "synced".to_string(),
            origin.path().display().to_string(),
            Some("main".to_string()),
            60,
            quadlet_dir.path(),
        )
        .await
        .unwrap();

        mgr.edit(
            "synced",
            origin.path().display().to_string(),
            None,
            5,
            quadlet_dir.path(),
        )
        .await
        .unwrap();

        let snap = mgr.snapshot();
        assert_eq!(snap[0].0.poll_interval_secs, 5);
        assert_eq!(snap[0].0.branch.as_deref(), Some("main"));

        // Persisted too, not just the in-memory entry.
        let reloaded =
            crate::config::Config::load(Some(config_dir.path().join("config.toml"))).unwrap();
        assert_eq!(reloaded.git_syncs[0].poll_interval_secs, 5);
    }

    #[tokio::test]
    async fn edit_switches_branch_and_repoints_the_checkout() {
        let origin = make_origin();
        assert!(
            git(origin.path(), &["checkout", "--quiet", "-b", "staging"])
                .status
                .success()
        );
        std::fs::write(
            origin.path().join("web.container"),
            "[Container]\nImage=staging\n",
        )
        .unwrap();
        assert!(
            git(
                origin.path(),
                &["commit", "--quiet", "-am", "staging build"]
            )
            .status
            .success()
        );

        let quadlet_dir = tempfile::tempdir().unwrap();
        let (mgr, _config_dir) = manager();
        mgr.add(
            "synced".to_string(),
            origin.path().display().to_string(),
            Some("main".to_string()),
            60,
            quadlet_dir.path(),
        )
        .await
        .unwrap();

        mgr.edit(
            "synced",
            origin.path().display().to_string(),
            Some("staging".to_string()),
            60,
            quadlet_dir.path(),
        )
        .await
        .unwrap();

        assert_eq!(mgr.snapshot()[0].0.branch.as_deref(), Some("staging"));
        assert_eq!(
            std::fs::read_to_string(quadlet_dir.path().join("synced/web.container")).unwrap(),
            "[Container]\nImage=staging\n"
        );
    }
}
