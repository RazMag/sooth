//! Reads a unit's journal by shelling out to `journalctl --user`. This is the
//! one deliberate exception to "D-Bus for everything" in this app: the
//! journal isn't usefully exposed over D-Bus for streaming, and binding
//! `sd-journal` over FFI isn't justified just to tail recent log lines.
//!
//! Safety note: `service` here must always be a systemd unit name derived by
//! this app (`QuadletUnit::service_name()`), never raw user input passed
//! straight through -- callers must not build these commands from anything
//! else, which forecloses argument injection.

use tokio::io::BufReader;
use tokio::process::{Child, ChildStdout, Command};

use crate::systemd::SystemdError;

/// Reads the most recent `lines` lines of a unit's journal for the initial
/// page load.
pub async fn tail_recent(service: &str, lines: u32) -> Result<String, SystemdError> {
    let output = Command::new("journalctl")
        .args([
            "--user",
            "-u",
            service,
            "-n",
            &lines.to_string(),
            "--no-pager",
            "-o",
            "short-iso",
        ])
        .output()
        .await
        .map_err(|e| SystemdError::action_failed(service, "read logs", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SystemdError::action_failed(service, "read logs", stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Spawns `journalctl --user -f` for live tailing. The child is configured
/// to be killed when dropped, so the caller only needs to keep it alive for
/// as long as it wants the tail to keep running (e.g. for the lifetime of an
/// SSE connection).
pub fn follow(service: &str) -> std::io::Result<(Child, BufReader<ChildStdout>)> {
    let mut child = Command::new("journalctl")
        .args([
            "--user",
            "-u",
            service,
            "-f",
            "-o",
            "short-iso",
            "--since",
            "now",
        ])
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("child stdout was not piped (this is a bug)"))?;
    Ok((child, BufReader::new(stdout)))
}
