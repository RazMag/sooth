//! Git-synced quadlet groups: a group directory whose contents are kept in
//! sync with a remote git repository, checked and pulled automatically on a
//! per-sync interval. Auth is left entirely to the host's own `git`
//! configuration (SSH agent, `~/.ssh/config`, credential helpers) -- sooth
//! stores no credentials of its own, it just runs `git` as the same user.
//!
//! See [`GitSyncManager`] for the runtime side (one poll task per configured
//! sync) and `crate::web::handlers::gitsync` for the web surface. Deliberately
//! does *not* trigger a systemd reload or `UnitsChanged` broadcast itself --
//! a sync's writes into the quadlet tree are indistinguishable from a human
//! editing files there, so the existing `quadlet::discovery::watch` +
//! `main.rs` fs-watch task already reloads and refreshes the UI for free.

pub mod git;
pub mod manager;

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub use manager::GitSyncManager;

fn default_poll_interval_secs() -> u64 {
    60
}

/// Persisted configuration for one git-synced group, round-tripped through
/// `Config.git_syncs` (a `[[git_syncs]]` TOML array of tables). The group
/// path doubles as the unique key -- a folder can only sensibly mirror one
/// repo at a time, and groups are already addressed by path everywhere else
/// (see `quadlet::naming::valid_group`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSyncConfig {
    pub group: String,
    pub remote: String,
    /// `None` only until the first successful clone resolves the remote's
    /// default branch and `GitSyncManager::add` patches it back in -- from
    /// then on always `Some`, so every later fetch/compare targets an
    /// explicit ref rather than re-guessing it.
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum GitSyncError {
    #[error("git-sync '{0}' not found")]
    NotFound(String),
    #[error("a git-sync for group '{0}' already exists")]
    AlreadyExists(String),
    #[error("{0}")]
    Validation(String),
    #[error("git is not installed or not on PATH")]
    GitNotFound,
    #[error("git command timed out")]
    Timeout,
    #[error("git failed: {0}")]
    Failed(String),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
}

impl GitSyncError {
    /// True for errors that mean "what you submitted was bad" -- callers
    /// redisplay the originating form with the message rather than a generic
    /// error page. Mirrors `QuadletError::is_client_error`.
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            GitSyncError::Validation(_)
                | GitSyncError::AlreadyExists(_)
                | GitSyncError::NotFound(_)
        )
    }
}

/// A sync's live progress/outcome, held per-entry in `GitSyncManager` and
/// rendered on the Git Sync page. Distinct from `GitSyncError`: most of
/// these states aren't errors, and `Error` here is already user-facing text
/// (not a `GitSyncError`) so a status snapshot stays plain `Clone` data with
/// no need to hold the entry's lock while rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncState {
    Cloning,
    Checking,
    Syncing,
    UpToDate {
        commit: String,
    },
    /// The checkout has diverged from the remote (or some other attempt
    /// failed) and needs attention -- see `GitSyncManager::force_resync`.
    Error(String),
}

#[derive(Debug, Clone)]
pub struct SyncStatus {
    pub checked_at: Option<SystemTime>,
    pub state: SyncState,
}

impl Default for SyncStatus {
    fn default() -> Self {
        Self {
            checked_at: None,
            state: SyncState::Checking,
        }
    }
}
