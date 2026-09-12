//! The event types pushed onto the app-wide broadcast channel that drives
//! live updates in the web UI (via Server-Sent Events). Kept independent of
//! both `systemd` and `web` so neither has to depend on the other just to
//! describe "something changed".

use crate::systemd::UnitStatus;

#[derive(Debug, Clone)]
pub enum DashboardEvent {
    /// A unit's live systemd status changed, keyed by systemd service name
    /// (e.g. `myapp.service`).
    Status { service: String, status: UnitStatus },
    /// The set of quadlet files on disk changed -- created, edited, or
    /// deleted, whether through the app or externally.
    UnitsChanged,
    /// A git-sync's status changed (cloning, checking, synced, errored) or
    /// the set of configured syncs changed (added/removed). Separate from
    /// `UnitsChanged`: a sync's own writes into the quadlet tree already
    /// drive that event through the ordinary fs-watch path (see
    /// `quadlet::gitsync`), this one is just for the Git Sync page's own
    /// status table.
    GitSyncChanged,
}

pub type EventSender = tokio::sync::broadcast::Sender<DashboardEvent>;
