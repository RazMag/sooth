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
}

pub type EventSender = tokio::sync::broadcast::Sender<DashboardEvent>;
