//! Podman secrets -- the values a quadlet consumes through `Secret=`
//! (`crate::quadlet::refs::secret_refs`). sooth keeps no secret store of its
//! own: this shells out to `podman secret` (the app's third shell-out, after
//! `journal.rs` and `quadlet::gitsync::git`), so values live wherever
//! podman's configured driver keeps them and a git-synced repo only ever
//! needs to carry secret *names*.
//!
//! Values go in via [`set`], piped to podman on stdin (never argv, never a
//! temp file, never logged). The only way one comes back out is [`reveal`],
//! driven by an explicit, CSRF-checked "Show" click on the Secrets page --
//! never rendered into a page by default, and never cached by the browser.
//!
//! Safety note: every `name` passed to a command here must first pass
//! [`valid_name`], which rejects a leading `-`, so it can't be parsed as a
//! flag.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(15);

/// Podman's own cap on a secret's size (512 KiB).
pub const MAX_VALUE_BYTES: usize = 512 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("{0}")]
    Validation(String),
    #[error("podman is not installed or not on PATH")]
    PodmanNotFound,
    #[error("podman secret command timed out")]
    Timeout,
    #[error("podman secret failed: {0}")]
    Failed(String),
    #[error("i/o error talking to podman: {0}")]
    Io(#[from] std::io::Error),
}

impl SecretsError {
    /// Mirrors `QuadletError::is_client_error`: the submission was bad, so
    /// redisplay the form rather than a generic error page.
    pub fn is_client_error(&self) -> bool {
        matches!(self, SecretsError::Validation(_))
    }
}

/// One entry from `podman secret ls`. No value -- see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretInfo {
    pub name: String,
    pub driver: String,
    /// Podman's own humanized timestamps ("3 hours ago").
    pub created: String,
    pub updated: String,
}

/// Podman's secret-name rule: `[A-Za-z0-9][A-Za-z0-9_.-]*`, at most 253
/// bytes. The leading-alphanumeric requirement is also what keeps a name
/// from being read as a CLI flag.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    name.len() <= 253 && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn base_command() -> Command {
    let mut cmd = Command::new("podman");
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

fn spawn_error(e: std::io::Error) -> SecretsError {
    if e.kind() == std::io::ErrorKind::NotFound {
        SecretsError::PodmanNotFound
    } else {
        SecretsError::Io(e)
    }
}

fn check(output: std::process::Output) -> Result<String, SecretsError> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(SecretsError::Failed(if stderr.is_empty() {
            format!("podman exited with {}", output.status)
        } else {
            stderr.trim_start_matches("Error: ").to_string()
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn run(mut cmd: Command) -> Result<String, SecretsError> {
    cmd.stdin(Stdio::null());
    let output = tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .map_err(|_| SecretsError::Timeout)?
        .map_err(spawn_error)?;
    check(output)
}

/// Every secret in this user's podman store, sorted by name.
pub async fn list() -> Result<Vec<SecretInfo>, SecretsError> {
    let mut cmd = base_command();
    cmd.args([
        "secret",
        "ls",
        "--noheading",
        "--format",
        "{{.Name}}\t{{.Driver}}\t{{.CreatedAt}}\t{{.UpdatedAt}}",
    ]);
    let mut out = parse_list(&run(cmd).await?);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn parse_list(stdout: &str) -> Vec<SecretInfo> {
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut f = line.split('\t').map(str::trim);
            SecretInfo {
                name: f.next().unwrap_or_default().to_string(),
                driver: f.next().unwrap_or_default().to_string(),
                created: f.next().unwrap_or_default().to_string(),
                updated: f.next().unwrap_or_default().to_string(),
            }
        })
        .filter(|s| !s.name.is_empty())
        .collect()
}

/// Just the names, for existence checks against `refs::secret_refs`.
pub async fn names() -> Result<std::collections::HashSet<String>, SecretsError> {
    Ok(list().await?.into_iter().map(|s| s.name).collect())
}

/// Creates secret `name` with `value`, or overwrites it when `replace`
/// (`podman secret create --replace`). Without `replace`, an existing name
/// is a `Validation` error rather than a silent overwrite.
pub async fn set(name: &str, value: &[u8], replace: bool) -> Result<(), SecretsError> {
    if !valid_name(name) {
        return Err(SecretsError::Validation(
            "Name must start with a letter or digit and contain only letters, digits, \
             '_', '.', and '-'."
                .into(),
        ));
    }
    if value.is_empty() {
        return Err(SecretsError::Validation("Value must not be empty.".into()));
    }
    if value.len() > MAX_VALUE_BYTES {
        return Err(SecretsError::Validation(
            "Value is larger than podman's 512 KiB limit.".into(),
        ));
    }

    let mut cmd = base_command();
    cmd.arg("secret").arg("create");
    if replace {
        cmd.arg("--replace");
    }
    cmd.arg(name).arg("-").stdin(Stdio::piped());

    let mut child = cmd.spawn().map_err(spawn_error)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("child stdin was not piped (this is a bug)"))?;
    let output = tokio::time::timeout(TIMEOUT, async {
        stdin.write_all(value).await?;
        // Close stdin so podman sees EOF and finishes reading the value.
        drop(stdin);
        child.wait_with_output().await
    })
    .await
    .map_err(|_| SecretsError::Timeout)??;

    check(output).map(drop).map_err(|e| match e {
        SecretsError::Failed(msg) if msg.contains("name in use") => {
            SecretsError::Validation(format!("A secret named '{name}' already exists."))
        }
        e => e,
    })
}

/// A secret's current value (`podman secret inspect --showsecret`), for the
/// Secrets page's on-demand "Show". Lossy UTF-8 -- it's for a human to read.
pub async fn reveal(name: &str) -> Result<String, SecretsError> {
    if !valid_name(name) {
        return Err(SecretsError::Validation("Invalid secret name.".into()));
    }
    let mut cmd = base_command();
    cmd.args([
        "secret",
        "inspect",
        "--showsecret",
        "--format",
        "{{.SecretData}}",
        name,
    ]);
    Ok(strip_format_newline(run(cmd).await?))
}

/// Drops the one newline podman's `--format` template appends -- and only
/// that one, so a value that itself ends in a newline keeps it.
fn strip_format_newline(mut out: String) -> String {
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// `podman secret rm <name>`. Callers are expected to have checked that no
/// quadlet still references it (see `web::handlers::secrets::remove`).
pub async fn remove(name: &str) -> Result<(), SecretsError> {
    if !valid_name(name) {
        return Err(SecretsError::Validation("Invalid secret name.".into()));
    }
    let mut cmd = base_command();
    cmd.args(["secret", "rm", name]);
    run(cmd).await.map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_rules() {
        assert!(valid_name("db-pass"));
        assert!(valid_name("a"));
        assert!(valid_name("0.tls_key-v2"));
        assert!(!valid_name(""));
        assert!(!valid_name("-rf"));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("has space"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a,type=env"));
        assert!(!valid_name(&"a".repeat(254)));
        assert!(valid_name(&"a".repeat(253)));
    }

    #[test]
    fn strips_only_the_format_newline() {
        assert_eq!(strip_format_newline("s3cret\n".into()), "s3cret");
        assert_eq!(
            strip_format_newline("line1\nline2\n\n".into()),
            "line1\nline2\n"
        );
        assert_eq!(strip_format_newline("no-newline".into()), "no-newline");
        assert_eq!(strip_format_newline(String::new()), "");
    }

    #[test]
    fn parses_ls_output() {
        let out = "db-pass\tfile\t2 hours ago\tAbout a minute ago\n\ntls\tpass\tx\ty\n";
        let parsed = parse_list(out);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "db-pass");
        assert_eq!(parsed[0].driver, "file");
        assert_eq!(parsed[0].updated, "About a minute ago");
        assert_eq!(parsed[1].driver, "pass");
    }
}
