//! The hub's watch on its machines: every [`INTERVAL`] it asks each paired
//! machine about itself ([`Fleet::check_all`]), so the sidebar's dots and the
//! Machines page know who's up without waiting for someone to open a page
//! that talks to them. A machine coming or going is announced by
//! [`Fleet::check`] itself.

use std::time::Duration;

use tokio::task::JoinHandle;

use super::Fleet;

pub const INTERVAL: Duration = Duration::from_secs(30);

/// Starts the watch. The first round runs at once, so the dots fill in
/// shortly after the hub starts.
pub fn spawn(fleet: Fleet) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if !fleet.machines().is_empty() {
                fleet.check_all().await;
            }
            tokio::time::sleep(INTERVAL).await;
        }
    })
}
