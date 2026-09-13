//! Self-updating: checks GitHub Releases for a newer `sooth` binary and,
//! depending on [`UpdateMode`], either just reports it or downloads it ready
//! to install. The GitHub API/checksum/download/swap work is all delegated
//! to the [`self_update`] crate (which uses
//! [`self_replace`](https://crates.io/crates/self-replace) internally for the
//! actual file swap) -- this module is the sooth-specific orchestration
//! around it: config, live status, and the one background poll task. See
//! [`SelfUpdateManager`] for the runtime side and
//! `crate::web::handlers::selfupdate` for the web surface.
//!
//! Applying an update only ever replaces the binary on disk; it never
//! restarts the process itself. `Auto` mode calls
//! `AppState::restart.notify_one()` right after downloading; `Notify` mode
//! stops at [`UpdateState::ReadyToRestart`] and leaves that to the Settings
//! page's existing "Restart" button, reusing the exact restart/re-exec
//! plumbing either way -- see the Gotchas note in `AGENTS.md` on why
//! `main::reexec` must never call `std::env::current_exe()` again after this
//! module has replaced the running binary's file.

pub mod manager;

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub use manager::SelfUpdateManager;

fn default_poll_interval_secs() -> u64 {
    21_600 // 6 hours
}

fn default_mode() -> UpdateMode {
    UpdateMode::Off
}

fn default_repo() -> String {
    "RazMag/sooth".to_string()
}

/// Whether/how sooth checks for and applies its own updates -- mirrors the
/// `AutoUpdate=off|registry|local` control sooth already exposes per-container
/// (`quadlet::autoupdate`), just for sooth itself: `Off` (the default), or
/// `Notify` (check on schedule and surface "update available" for a human to
/// download and install), or `Auto` (check on schedule and install without a
/// click).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    Off,
    /// Check on schedule and surface "update available"; a human clicks
    /// "Download update", then separately "Install and restart" once it's
    /// ready.
    Notify,
    /// Check on schedule and download + install immediately, no click
    /// required.
    Auto,
}

impl UpdateMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "notify" => Some(Self::Notify),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Notify => "notify",
            Self::Auto => "auto",
        }
    }
}

/// Persisted self-update configuration, round-tripped through
/// `Config.self_update` (a `[self_update]` TOML table). `repo` keeps this
/// host-configurable exactly like `GitSyncConfig.remote`, so a fork can point
/// self-update at its own GitHub repo instead of upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfUpdateConfig {
    #[serde(default = "default_mode")]
    pub mode: UpdateMode,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
    /// `"owner/name"` on GitHub. Defaults to upstream; a fork building its
    /// own releases points this at itself instead.
    #[serde(default = "default_repo")]
    pub repo: String,
}

impl Default for SelfUpdateConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            poll_interval_secs: default_poll_interval_secs(),
            repo: default_repo(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SelfUpdateError {
    #[error("self-update is turned off")]
    Disabled,
    #[error("no update is currently recorded as available")]
    NoUpdateAvailable,
    #[error("an update check or download is already in progress")]
    Busy,
    #[error("{0}")]
    Validation(String),
    #[error("failed to save config: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Update(#[from] self_update::errors::Error),
}

impl SelfUpdateError {
    /// True for errors that mean "what you submitted was bad" -- callers
    /// redisplay the originating form with the message rather than a generic
    /// error page. Mirrors `GitSyncError::is_client_error`.
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            SelfUpdateError::Disabled
                | SelfUpdateError::NoUpdateAvailable
                | SelfUpdateError::Busy
                | SelfUpdateError::Validation(_)
        )
    }
}

/// Splits `"owner/name"` into its two parts, as `self_update`'s GitHub
/// backend wants them. Shared by the manager (building the updater on every
/// attempt) and the Settings form handler (rejecting a malformed `repo` at
/// save time instead of waiting for the next poll to discover it).
pub(crate) fn split_repo(repo: &str) -> Result<(String, String), SelfUpdateError> {
    match repo.split_once('/') {
        Some((owner, name)) if !owner.is_empty() && !name.is_empty() && !name.contains('/') => {
            Ok((owner.to_string(), name.to_string()))
        }
        _ => Err(SelfUpdateError::Validation(format!(
            "repo must look like \"owner/name\", got \"{repo}\""
        ))),
    }
}

/// Live progress/outcome of the self-update poll task, rendered on the
/// Settings page. Distinct from `SelfUpdateError`: most of these states
/// aren't errors, and `Error` here is already user-facing text, same
/// reasoning as `gitsync::SyncState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    /// A newer release exists but hasn't been downloaded yet -- `Notify`
    /// mode landed here and is waiting for "Download update".
    UpdateAvailable {
        version: String,
    },
    Downloading,
    /// Downloaded, verified, and swapped onto disk; the running process is
    /// still the old code until a restart. Only reached in `Notify` mode --
    /// `Auto` mode goes straight from `Downloading` to notifying `restart`
    /// itself, no click needed.
    ReadyToRestart {
        version: String,
    },
    /// `Auto` mode only: downloaded and about to restart.
    Applying,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct UpdateStatus {
    pub checked_at: Option<SystemTime>,
    pub state: UpdateState,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            checked_at: None,
            state: UpdateState::Idle,
        }
    }
}
