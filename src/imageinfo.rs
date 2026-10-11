//! Reads an image's declared `EXPOSE` ports via `podman image inspect` --
//! used by the Ports screen to tell which container in a pod a pod-published
//! port is meant for. Best-effort by design: an image that isn't pulled yet,
//! a missing podman, or a slow call just yields `None`, and the caller falls
//! back to showing every pod member (with a `pull` button, since an image a
//! quadlet names is only pulled when its unit first starts).
//!
//! Also answers "is there a newer image in the registry?" for a container's
//! row-menu update check ([`check_update`]): the locally stored image's
//! digests against the registry's current manifest, read with
//! `podman manifest inspect` -- no layers are downloaded until the user
//! actually asks for the update (an ordinary [`pull`]).

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::quadlet::ports::{self, ExposedPort};

const TIMEOUT: Duration = Duration::from_secs(5);
/// Pulls are network-bound and images can be large.
const PULL_TIMEOUT: Duration = Duration::from_secs(600);
/// A registry round trip (auth + manifest), not a download.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(30);

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

/// The outcome of [`check_update`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// The local image is what the registry currently serves for its tag.
    UpToDate,
    /// The registry has a different image for the tag.
    Available,
    /// Nothing in local storage yet -- a pull would fetch it.
    NotPulled,
    /// No registry to compare against: a digest-pinned or `localhost/`
    /// image. The text says which.
    Unsupported(&'static str),
    /// podman or the registry failed; podman's own message.
    Failed(String),
}

/// Whether the registry has a newer image for `image` than local storage.
/// Compares the local image's recorded digests (and ID) against the
/// registry's current manifest for the same tag -- for a multi-arch index,
/// any per-platform manifest the local image was pulled as counts as a
/// match; for a single manifest, its config digest must be the local ID.
pub async fn check_update(image: &str) -> UpdateCheck {
    if image.is_empty() || image.starts_with('-') {
        return UpdateCheck::Failed(format!("not a pullable image reference: {image}"));
    }
    if image.contains('@') {
        return UpdateCheck::Unsupported("pinned to a digest");
    }
    let local = match inspect_local(image).await {
        Ok(Some(l)) => l,
        Ok(None) => return UpdateCheck::NotPulled,
        Err(e) => return UpdateCheck::Failed(e),
    };
    let reference = match qualify(image, &local.repo_tags) {
        Ok(r) => r,
        Err(why) => return UpdateCheck::Unsupported(why),
    };
    let remote = match podman_json(&["manifest", "inspect", &reference], REMOTE_TIMEOUT).await {
        Ok(v) => v,
        Err(e) => return UpdateCheck::Failed(e),
    };
    match matches_remote(&local, &remote) {
        Some(true) => UpdateCheck::UpToDate,
        Some(false) => UpdateCheck::Available,
        None => UpdateCheck::Failed("unrecognised registry manifest".to_string()),
    }
}

/// What [`check_update`] needs from `podman image inspect`.
#[derive(Debug, Default)]
struct LocalImage {
    /// The config digest, bare hex (podman's `Id`).
    id: String,
    /// Every manifest digest the image is known by (`sha256:…`).
    digests: Vec<String>,
    repo_tags: Vec<String>,
}

/// `Ok(None)` when the image isn't in local storage.
async fn inspect_local(image: &str) -> Result<Option<LocalImage>, String> {
    let mut cmd = Command::new("podman");
    cmd.args(["image", "exists", image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(TIMEOUT, cmd.status()).await {
        Ok(Ok(s)) if s.code() == Some(1) => return Ok(None),
        Ok(Ok(_)) => {}
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("podman not found".to_string());
        }
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => return Err("podman image exists timed out".to_string()),
    }
    let json = podman_json(&["image", "inspect", image], TIMEOUT).await?;
    Ok(Some(parse_local(&json)))
}

fn parse_local(json: &serde_json::Value) -> LocalImage {
    let image = json.get(0).unwrap_or(json);
    let strings = |key: &str| -> Vec<String> {
        image
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut digests: Vec<String> = strings("RepoDigests")
        .iter()
        .filter_map(|d| d.split_once('@').map(|(_, digest)| digest.to_string()))
        .collect();
    if let Some(d) = image.get("Digest").and_then(|v| v.as_str()) {
        digests.push(d.to_string());
    }
    LocalImage {
        id: image
            .get("Id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim_start_matches("sha256:")
            .to_string(),
        digests,
        repo_tags: strings("RepoTags"),
    }
}

/// The reference to ask the registry about. A fully qualified `image` is
/// used as is; a short name (`nginx:alpine`) becomes the local tag it was
/// resolved to when pulled (`docker.io/library/nginx:alpine`), so the check
/// asks the same registry -- falling back to the short name, which podman
/// resolves itself. Images with no remote registry are rejected.
fn qualify(image: &str, repo_tags: &[String]) -> Result<String, &'static str> {
    let first = image.split('/').next().unwrap_or_default();
    let has_registry =
        image.contains('/') && (first.contains('.') || first.contains(':') || first == "localhost");
    if first == "localhost" && has_registry {
        return Err("a local image");
    }
    if has_registry {
        return Ok(image.to_string());
    }
    let tagged = if image.rsplit('/').next().is_some_and(|n| n.contains(':')) {
        image.to_string()
    } else {
        format!("{image}:latest")
    };
    let suffix = format!("/{tagged}");
    match repo_tags.iter().find(|t| t.ends_with(&suffix)) {
        Some(t) if t.starts_with("localhost/") => Err("a local image"),
        Some(t) => Ok(t.clone()),
        None => Ok(image.to_string()),
    }
}

/// `Some(true)` when the registry's manifest is one the local image was
/// pulled as; `None` when the manifest has neither shape.
fn matches_remote(local: &LocalImage, remote: &serde_json::Value) -> Option<bool> {
    if let Some(list) = remote.get("manifests").and_then(|m| m.as_array()) {
        return Some(list.iter().any(|m| {
            m.get("digest")
                .and_then(|d| d.as_str())
                .is_some_and(|d| local.digests.iter().any(|l| l == d))
        }));
    }
    let config = remote.get("config")?.get("digest")?.as_str()?;
    Some(config.trim_start_matches("sha256:") == local.id)
}

/// Runs a podman subcommand that prints JSON, returning it parsed or
/// podman's last stderr line.
async fn podman_json(args: &[&str], timeout: Duration) -> Result<serde_json::Value, String> {
    let mut cmd = Command::new("podman");
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => {
            let what: Vec<&str> = args.iter().take(2).copied().collect();
            return Err(format!("podman {} timed out", what.join(" ")));
        }
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("podman not found".to_string());
        }
        Ok(Err(e)) => return Err(e.to_string()),
        Ok(Ok(o)) => o,
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("podman failed")
            .trim()
            .trim_start_matches("Error: ")
            .to_string();
        tracing::debug!(?args, error = msg, "podman command failed");
        return Err(msg);
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("unreadable podman output: {e}"))
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

    fn tags(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn qualifies_short_names_from_local_tags() {
        let local = tags(&["docker.io/library/nginx:alpine"]);
        assert_eq!(
            qualify("nginx:alpine", &local).unwrap(),
            "docker.io/library/nginx:alpine"
        );
        let local = tags(&["docker.io/library/redis:latest"]);
        assert_eq!(
            qualify("redis", &local).unwrap(),
            "docker.io/library/redis:latest"
        );
        // No matching tag: podman resolves the short name itself.
        assert_eq!(qualify("redis", &[]).unwrap(), "redis");
        // `nginx` must not match `docker.io/foo/xnginx:latest`.
        let local = tags(&["docker.io/foo/xnginx:latest"]);
        assert_eq!(qualify("nginx", &local).unwrap(), "nginx");
    }

    #[test]
    fn qualified_names_pass_through_and_local_ones_are_rejected() {
        assert_eq!(
            qualify("ghcr.io/org/app:1", &[]).unwrap(),
            "ghcr.io/org/app:1"
        );
        assert_eq!(
            qualify("registry:5000/app", &[]).unwrap(),
            "registry:5000/app"
        );
        assert!(qualify("localhost/app:latest", &[]).is_err());
        assert!(qualify("app", &tags(&["localhost/app:latest"])).is_err());
    }

    #[test]
    fn parses_local_inspect() {
        let json = serde_json::json!([{
            "Id": "abc",
            "Digest": "sha256:list",
            "RepoTags": ["docker.io/library/nginx:alpine"],
            "RepoDigests": ["docker.io/library/nginx@sha256:one", "docker.io/library/nginx@sha256:list"],
        }]);
        let l = parse_local(&json);
        assert_eq!(l.id, "abc");
        assert_eq!(l.digests, ["sha256:one", "sha256:list", "sha256:list"]);
        assert_eq!(l.repo_tags, ["docker.io/library/nginx:alpine"]);
    }

    #[test]
    fn compares_against_an_index() {
        let local = LocalImage {
            id: "cfg".into(),
            digests: vec!["sha256:amd64".into()],
            repo_tags: vec![],
        };
        let current = serde_json::json!({"manifests": [
            {"digest": "sha256:arm64"}, {"digest": "sha256:amd64"},
        ]});
        assert_eq!(matches_remote(&local, &current), Some(true));
        let newer = serde_json::json!({"manifests": [{"digest": "sha256:new"}]});
        assert_eq!(matches_remote(&local, &newer), Some(false));
    }

    #[test]
    fn compares_a_single_manifest_by_config_digest() {
        let local = LocalImage {
            id: "cfg".into(),
            ..Default::default()
        };
        let same = serde_json::json!({"config": {"digest": "sha256:cfg"}});
        assert_eq!(matches_remote(&local, &same), Some(true));
        let newer = serde_json::json!({"config": {"digest": "sha256:other"}});
        assert_eq!(matches_remote(&local, &newer), Some(false));
        assert_eq!(matches_remote(&local, &serde_json::json!({})), None);
    }
}
