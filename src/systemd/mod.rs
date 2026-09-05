pub mod client;
pub mod status;
pub mod watch;

pub use client::Client;
pub use status::UnitStatus;

#[derive(Debug, thiserror::Error)]
pub enum SystemdError {
    #[error("failed to connect to the systemd user session bus: {0}")]
    Connection(#[from] zbus::Error),
    #[error("action '{action}' failed for unit '{unit}': {detail}")]
    ActionFailed { unit: String, action: String, detail: String },
}

impl SystemdError {
    pub(crate) fn action_failed(unit: &str, action: &str, e: impl std::fmt::Display) -> Self {
        SystemdError::ActionFailed { unit: unit.to_string(), action: action.to_string(), detail: e.to_string() }
    }
}
