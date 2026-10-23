//! Container registry logins -- the credentials podman pulls a private image
//! with. sooth keeps no store of its own: logging in shells out to
//! `podman login`, logging out to `podman logout`, both against
//! [`auth_file`] (`~/.config/containers/auth.json`).
//!
//! That file, not podman's default, is the point. A bare `podman login`
//! writes `$XDG_RUNTIME_DIR/containers/auth.json`, which is on tmpfs: the
//! container pulls fine until the first reboot, then its service fails to
//! start. Rootless podman always falls back to the config-dir file, so a
//! login saved there works for the generated units *and* for sooth's own
//! `podman pull` / `manifest inspect` / `search` calls (`crate::imageinfo`),
//! none of which pass `--authfile`.
//!
//! The runtime file is still read, since podman checks it *first*: an entry
//! there shadows the saved one for the same registry. [`list`] shows those
//! as session-only, and [`login`] drops a shadowing entry once the new
//! credentials have been accepted, so a stale session login can't keep
//! winning.
//!
//! Passwords go to podman on stdin (`--password-stdin`; never argv, never a
//! temp file, never logged) and are never read back -- [`list`] reads only
//! the file's registry keys, and the username comes from
//! `podman login --get-login`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// `login` contacts the registry to check the credentials, so it gets more
/// room than the purely local commands.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{0}")]
    Validation(String),
    /// podman (or the registry) turned the login down -- bad credentials, an
    /// unreachable host. Shown on the form, like `Validation`.
    #[error("{0}")]
    Rejected(String),
    #[error("podman is not installed or not on PATH")]
    PodmanNotFound,
    #[error("podman timed out")]
    Timeout,
    #[error("podman failed: {0}")]
    Failed(String),
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("could not parse {path}: {source}")]
    Parse {
        path: String,
        source: serde_json::Error,
    },
    #[error("could not locate the containers auth file (no config or runtime directory)")]
    NoAuthFile,
    #[error("i/o error talking to podman: {0}")]
    Io(#[from] std::io::Error),
}

impl RegistryError {
    /// Mirrors `SecretsError::is_client_error`: redisplay the form rather
    /// than a generic error page.
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            RegistryError::Validation(_) | RegistryError::Rejected(_)
        )
    }
}

/// One registry this user is logged in to. Never carries a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryLogin {
    /// The key as stored in the auth file: a host (`ghcr.io`,
    /// `registry.example.com:5000`), optionally with a namespace
    /// (`ghcr.io/myorg`) -- podman scopes a login to the longest match.
    pub registry: String,
    /// `None` when podman can't name one (a credential-helper or token-only
    /// entry).
    pub username: Option<String>,
    /// In [`auth_file`] -- survives a reboot.
    pub saved: bool,
    /// In [`session_auth_file`] -- gone at reboot, and wins over `saved`
    /// while it lasts.
    pub session: bool,
}

/// Where sooth saves logins: `~/.config/containers/auth.json`
/// (`$XDG_CONFIG_HOME` respected, as podman does).
pub fn auth_file() -> Result<PathBuf, RegistryError> {
    dirs::config_dir()
        .map(|d| d.join("containers").join("auth.json"))
        .ok_or(RegistryError::NoAuthFile)
}

/// podman's default, `$XDG_RUNTIME_DIR/containers/auth.json`. `None` without
/// a runtime dir.
pub fn session_auth_file() -> Option<PathBuf> {
    dirs::runtime_dir().map(|d| d.join("containers").join("auth.json"))
}

/// What a registry field may hold: a host, optional `:port`, optional
/// `/namespace` path. The leading letter/digit (or `[` for an IPv6 literal)
/// also keeps it from being read as a flag.
pub fn valid_registry(registry: &str) -> bool {
    let mut chars = registry.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '[' => {}
        _ => return false,
    }
    registry.len() <= 255
        && chars.all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '/' | '[' | ']')
        })
}

/// Lets a pasted URL (`https://ghcr.io/`) stand for its registry: drops the
/// scheme and trailing slashes, then surrounding whitespace.
pub fn normalize_registry(raw: &str) -> &str {
    let s = raw.trim();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    s.trim_end_matches('/')
}

fn base_command() -> Command {
    let mut cmd = Command::new("podman");
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

fn spawn_error(e: std::io::Error) -> RegistryError {
    if e.kind() == std::io::ErrorKind::NotFound {
        RegistryError::PodmanNotFound
    } else {
        RegistryError::Io(e)
    }
}

/// podman's stderr, minus its `Error: ` prefix -- or the exit status when
/// it printed nothing.
fn failure_message(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("podman exited with {}", output.status)
    } else {
        stderr.trim_start_matches("Error: ").to_string()
    }
}

async fn run(mut cmd: Command) -> Result<String, RegistryError> {
    cmd.stdin(Stdio::null());
    let output = tokio::time::timeout(TIMEOUT, cmd.output())
        .await
        .map_err(|_| RegistryError::Timeout)?
        .map_err(spawn_error)?;
    if !output.status.success() {
        return Err(RegistryError::Failed(failure_message(&output)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The registry keys in an auth file, sorted. A missing file is no logins.
fn read_keys(path: &Path) -> Result<Vec<String>, RegistryError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(RegistryError::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    parse_keys(&text).map_err(|source| RegistryError::Parse {
        path: path.display().to_string(),
        source,
    })
}

/// Just the keys of `auths` -- the values (base64 `user:password`) are never
/// looked at.
fn parse_keys(text: &str) -> Result<Vec<String>, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct AuthFile {
        #[serde(default)]
        auths: BTreeMap<String, serde::de::IgnoredAny>,
    }
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let file: AuthFile = serde_json::from_str(text)?;
    Ok(file.auths.into_keys().collect())
}

/// Folds the two files' keys into one row per registry.
fn merge(saved: Vec<String>, session: Vec<String>) -> Vec<RegistryLogin> {
    let mut rows: BTreeMap<String, RegistryLogin> = BTreeMap::new();
    for (keys, is_session) in [(saved, false), (session, true)] {
        for registry in keys {
            let row = rows
                .entry(registry.clone())
                .or_insert_with(|| RegistryLogin {
                    registry,
                    username: None,
                    saved: false,
                    session: false,
                });
            if is_session {
                row.session = true;
            } else {
                row.saved = true;
            }
        }
    }
    rows.into_values().collect()
}

/// `podman login --get-login` against one file. Best-effort: an entry with
/// no username (or any failure) is `None`.
async fn get_login(file: &Path, registry: &str) -> Option<String> {
    let mut cmd = base_command();
    cmd.args(["login", "--get-login", "--authfile"])
        .arg(file)
        .arg("--")
        .arg(registry);
    let out = run(cmd).await.ok()?;
    Some(out.trim().to_string()).filter(|u| !u.is_empty())
}

/// Every registry logged in to, from the saved file and the session one.
/// The username shown is the effective one -- the session entry's when
/// there is one, since podman reads that file first.
pub async fn list() -> Result<Vec<RegistryLogin>, RegistryError> {
    let saved_file = auth_file()?;
    let session_file = session_auth_file();
    let saved = read_keys(&saved_file)?;
    let session = match &session_file {
        Some(f) => read_keys(f)?,
        None => Vec::new(),
    };
    let mut rows = merge(saved, session);
    let lookups = rows.iter().map(|row| {
        let file = match (&session_file, row.session) {
            (Some(f), true) => f.clone(),
            _ => saved_file.clone(),
        };
        let registry = row.registry.clone();
        async move { get_login(&file, &registry).await }
    });
    let names = futures_util::future::join_all(lookups).await;
    for (row, name) in rows.iter_mut().zip(names) {
        row.username = name;
    }
    Ok(rows)
}

/// What [`login`] did besides saving.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LoginOutcome {
    /// A session-only entry for the same registry was removed so it no
    /// longer shadows the saved one.
    pub cleared_session: bool,
}

/// `podman login` into [`auth_file`], password on stdin. podman checks the
/// credentials with the registry before saving, so a bad password or an
/// unreachable host is a [`RegistryError::Rejected`] and nothing is written.
/// Logging in again to a saved registry replaces its credentials.
pub async fn login(
    registry: &str,
    username: &str,
    password: &str,
) -> Result<LoginOutcome, RegistryError> {
    if !valid_registry(registry) {
        return Err(RegistryError::Validation(
            "Registry must be a host name, optionally with a :port and a /path \
             (e.g. ghcr.io or registry.example.com:5000)."
                .into(),
        ));
    }
    if username.is_empty() || username.chars().any(char::is_control) {
        return Err(RegistryError::Validation(
            "Username must not be empty or contain line breaks.".into(),
        ));
    }
    if password.is_empty() {
        return Err(RegistryError::Validation(
            "Password must not be empty.".into(),
        ));
    }
    if password.contains(['\r', '\n']) {
        return Err(RegistryError::Validation(
            "Password must be a single line.".into(),
        ));
    }

    let file = auth_file()?;
    let mut cmd = base_command();
    cmd.args(["login", "--authfile"])
        .arg(&file)
        // `=` form, so a username starting with `-` stays a value.
        .arg(format!("--username={username}"))
        .arg("--password-stdin")
        .arg("--")
        .arg(registry)
        .stdin(Stdio::piped());

    let mut child = cmd.spawn().map_err(spawn_error)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("child stdin was not piped (this is a bug)"))?;
    let output = tokio::time::timeout(LOGIN_TIMEOUT, async {
        stdin.write_all(password.as_bytes()).await?;
        // Close stdin so podman sees EOF and stops reading.
        drop(stdin);
        child.wait_with_output().await
    })
    .await
    .map_err(|_| RegistryError::Timeout)??;
    if !output.status.success() {
        return Err(RegistryError::Rejected(failure_message(&output)));
    }

    let mut outcome = LoginOutcome::default();
    if let Some(session) = session_auth_file()
        && read_keys(&session).is_ok_and(|keys| keys.iter().any(|k| k == registry))
    {
        match logout_from(&session, registry).await {
            Ok(()) => outcome.cleared_session = true,
            Err(e) => {
                tracing::warn!(registry, error = %e, "could not drop shadowing session login");
            }
        }
    }
    Ok(outcome)
}

async fn logout_from(file: &Path, registry: &str) -> Result<(), RegistryError> {
    let mut cmd = base_command();
    cmd.args(["logout", "--authfile"])
        .arg(file)
        .arg("--")
        .arg(registry);
    run(cmd).await.map(drop)
}

/// Removes `registry` from every file it's in, saved and session alike --
/// leaving either behind would keep the login working. Only a key one of
/// the files actually holds is accepted, so nothing else reaches podman.
pub async fn logout(registry: &str) -> Result<(), RegistryError> {
    let mut files = vec![auth_file()?];
    files.extend(session_auth_file());
    let mut found = false;
    for file in &files {
        if read_keys(file)?.iter().any(|k| k == registry) {
            found = true;
            logout_from(file, registry).await?;
        }
    }
    if found {
        Ok(())
    } else {
        Err(RegistryError::Validation(format!(
            "Not logged in to '{registry}'."
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_rules() {
        assert!(valid_registry("ghcr.io"));
        assert!(valid_registry("registry.example.com:5000"));
        assert!(valid_registry("ghcr.io/my-org"));
        assert!(valid_registry("localhost"));
        assert!(valid_registry("[::1]:5000"));
        assert!(!valid_registry(""));
        assert!(!valid_registry("-v"));
        assert!(!valid_registry("/ghcr.io"));
        assert!(!valid_registry("ghcr.io extra"));
        assert!(!valid_registry("ghcr.io\n"));
        assert!(!valid_registry("user@ghcr.io"));
        assert!(!valid_registry(&"a".repeat(256)));
    }

    #[test]
    fn normalizes_pasted_urls() {
        assert_eq!(normalize_registry("  https://ghcr.io/ "), "ghcr.io");
        assert_eq!(
            normalize_registry("http://localhost:5000"),
            "localhost:5000"
        );
        assert_eq!(normalize_registry("quay.io/org//"), "quay.io/org");
        assert_eq!(normalize_registry("docker.io"), "docker.io");
    }

    #[test]
    fn reads_only_the_auths_keys() {
        let text = r#"{
            "auths": {
                "registry.example.com": { "auth": "YWxpY2U6cHc=" },
                "ghcr.io/org": { "auth": "Ym9iOnB3" },
                "quay.io": {}
            },
            "credHelpers": { "gcr.io": "gcloud" }
        }"#;
        assert_eq!(
            parse_keys(text).unwrap(),
            ["ghcr.io/org", "quay.io", "registry.example.com"]
        );
        assert!(parse_keys("").unwrap().is_empty());
        assert!(parse_keys("{}").unwrap().is_empty());
        assert!(parse_keys("not json").is_err());
    }

    #[test]
    fn missing_file_is_no_logins() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_keys(&dir.path().join("auth.json")).unwrap().is_empty());
    }

    #[test]
    fn merges_saved_and_session_rows() {
        let rows = merge(
            vec!["a.io".into(), "both.io".into()],
            vec!["both.io".into(), "s.io".into()],
        );
        let flags: Vec<_> = rows
            .iter()
            .map(|r| (r.registry.as_str(), r.saved, r.session))
            .collect();
        assert_eq!(
            flags,
            [
                ("a.io", true, false),
                ("both.io", true, true),
                ("s.io", false, true)
            ]
        );
    }

    #[tokio::test]
    async fn login_validates_before_running_podman() {
        let bad = [
            ("-x", "u", "p"),
            ("ghcr.io", "", "p"),
            ("ghcr.io", "a\nb", "p"),
            ("ghcr.io", "u", ""),
            ("ghcr.io", "u", "two\nlines"),
        ];
        for (registry, user, pass) in bad {
            let err = login(registry, user, pass).await.unwrap_err();
            assert!(
                matches!(err, RegistryError::Validation(_)),
                "{registry:?}/{user:?}: {err}"
            );
        }
    }
}
