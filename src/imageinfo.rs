//! Reads an image's declared `EXPOSE` ports via `podman image inspect` --
//! used by the Ports screen to tell which container in a pod a pod-published
//! port is meant for. Best-effort by design: an image that isn't pulled yet,
//! a missing podman, or a slow call just yields `None`, and the caller falls
//! back to showing every pod member (with a `pull` button, since an image a
//! quadlet names is only pulled when its unit first starts).

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::quadlet::ports::{self, ExposedPort};

const TIMEOUT: Duration = Duration::from_secs(5);
/// Pulls are network-bound and images can be large.
const PULL_TIMEOUT: Duration = Duration::from_secs(600);

/// The image's `Config.ExposedPorts`, parsed. `None` when it can't be read
/// (not in local storage, podman missing or failing, timeout); `Some(empty)`
/// when the image exposes nothing.
pub async fn exposed_ports(image: &str) -> Option<Vec<ExposedPort>> {
    // Never let a quadlet value be read as a podman flag.
    if image.is_empty() || image.starts_with('-') {
        return None;
    }
    let mut cmd = Command::new("podman");
    cmd.args([
        "image",
        "inspect",
        "--format",
        "{{range $k, $v := .Config.ExposedPorts}}{{$k}} {{end}}",
        image,
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    let output = match tokio::time::timeout(TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) if o.status.success() => o,
        other => {
            tracing::debug!(image, ?other, "podman image inspect gave no exposed ports");
            return None;
        }
    };
    Some(parse(&String::from_utf8_lossy(&output.stdout)))
}

/// `podman pull <image>` into this user's storage. The error is podman's own
/// message, for showing inline.
pub async fn pull(image: &str) -> Result<(), String> {
    if image.is_empty() || image.starts_with('-') {
        return Err(format!("not a pullable image reference: {image}"));
    }
    let mut cmd = Command::new("podman");
    cmd.args(["pull", "--quiet", image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(PULL_TIMEOUT, cmd.output()).await {
        Err(_) => return Err("podman pull timed out".to_string()),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("podman not found".to_string());
        }
        Ok(Err(e)) => return Err(e.to_string()),
        Ok(Ok(o)) => o,
    };
    if output.status.success() {
        tracing::info!(image, "pulled image");
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let msg = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("podman pull failed")
        .trim()
        .trim_start_matches("Error: ");
    tracing::warn!(image, error = msg, "podman pull failed");
    Err(msg.to_string())
}

fn parse(stdout: &str) -> Vec<ExposedPort> {
    stdout
        .split_whitespace()
        .filter_map(ports::parse_exposed)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inspect_output() {
        let got = parse("80/tcp 53/udp \n");
        assert_eq!(got.len(), 2);
        assert!(parse("\n").is_empty());
    }
}
