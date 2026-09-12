use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

use crate::events::EventSender;
use crate::health::HealthCell;
use crate::quadlet::QuadletError;
use crate::quadlet::gitsync::{GitSyncConfig, GitSyncManager};
use crate::systemd::Client;

/// App configuration, layered (later wins) as: compiled-in defaults -> an
/// optional TOML file -> `SOOTH_`-prefixed environment variables. Env vars
/// winning last fits deploying `sooth` itself as a systemd user service with
/// secrets in an `EnvironmentFile=`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub bind_addr: SocketAddr,
    /// Overrides the auto-detected `$XDG_CONFIG_HOME/containers/systemd`.
    pub quadlet_dir: Option<PathBuf>,
    /// An argon2 password hash (see `sooth --hash-password`), never the
    /// plaintext password itself.
    pub auth_password_hash: String,
    /// Whether the session cookie requires HTTPS. Defaults to `false` for
    /// localhost development; set `true` when running behind a
    /// TLS-terminating reverse proxy or otherwise reachable off-box, since
    /// this app has no built-in TLS of its own.
    #[serde(default)]
    pub cookie_secure: bool,
    pub log_filter: Option<String>,
    #[serde(default = "default_idle_timeout")]
    pub session_idle_timeout_secs: u64,
    /// Group directories kept in sync with a remote git repository -- see
    /// `crate::quadlet::gitsync`. A `[[git_syncs]]` array of tables in the
    /// TOML file, managed live (no restart needed) by the Git Sync page
    /// rather than the rest of this struct's "edit and restart" fields.
    #[serde(default)]
    pub git_syncs: Vec<GitSyncConfig>,
}

fn default_idle_timeout() -> u64 {
    43_200 // 12 hours
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 8420)),
            quadlet_dir: None,
            auth_password_hash: String::new(),
            cookie_secure: false,
            log_filter: None,
            session_idle_timeout_secs: default_idle_timeout(),
            git_syncs: Vec::new(),
        }
    }
}

impl Config {
    // figment::Error is large (it carries rich context for a config error
    // message); that's fine here since this only ever runs once at startup,
    // not on a hot path.
    #[allow(clippy::result_large_err)]
    pub fn load(config_path: Option<PathBuf>) -> Result<Self, figment::Error> {
        let path = config_path.unwrap_or_else(default_config_path);
        Figment::new()
            .merge(Serialized::defaults(Config::default()))
            .merge(Toml::file(path))
            .merge(Env::prefixed("SOOTH_"))
            .extract()
    }

    pub fn resolved_quadlet_dir(&self) -> Result<PathBuf, QuadletError> {
        match &self.quadlet_dir {
            Some(dir) => Ok(dir.clone()),
            None => crate::quadlet::discovery::default_quadlet_dir(),
        }
    }

    /// Fails fast with a descriptive message rather than starting into a
    /// broken or insecure state.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.auth_password_hash.trim().is_empty() {
            anyhow::bail!(
                "no password hash configured -- set SOOTH_AUTH_PASSWORD_HASH \
                 (generate one with: sooth --hash-password)"
            );
        }
        argon2::PasswordHash::new(&self.auth_password_hash).map_err(|e| {
            anyhow::anyhow!("SOOTH_AUTH_PASSWORD_HASH is not a valid argon2 hash: {e}")
        })?;
        Ok(())
    }
}

pub fn default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("sooth")
        .join("config.toml")
}

/// Parses the config TOML file into a raw `toml::Table` (empty if it doesn't
/// exist yet) -- the starting point for a read-modify-write save that must
/// preserve keys the caller isn't touching. Shared by the Settings page
/// (`web::handlers::settings`, whole-value key updates) and git-sync
/// (`quadlet::gitsync::manager`, `git_syncs` array updates).
pub(crate) fn read_config_toml(path: &Path) -> std::io::Result<toml::Table> {
    match std::fs::read_to_string(path) {
        Ok(text) => text.parse().map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("existing config is not valid TOML: {e}"),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(e) => Err(e),
    }
}

/// Serializes `table` back to the config file, atomically (temp file in the
/// same directory, then rename).
pub(crate) fn write_config_toml(path: &Path, table: &toml::Table) -> std::io::Result<()> {
    let text = toml::to_string_pretty(table)
        .map_err(|e| std::io::Error::other(format!("failed to serialize config: {e}")))?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".sooth-settings-{}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, &text)?;
    std::fs::rename(&tmp, path)
}

/// Read-modify-write a handful of top-level scalar keys, leaving everything
/// else in the file untouched -- what the Settings page's "Save" buttons use
/// (bind address, quadlet dir, ..., the password hash).
pub(crate) fn patch_config_toml(
    path: &Path,
    updates: &[(&str, toml::Value)],
) -> std::io::Result<()> {
    let mut table = read_config_toml(path)?;
    for (key, value) in updates {
        table.insert((*key).to_string(), value.clone());
    }
    write_config_toml(path, &table)
}

/// Read-modify-write the whole `git_syncs` array of tables: deserializes it
/// into a `Vec<GitSyncConfig>` (empty if the key is absent), lets `mutate`
/// change the list, then writes it back as `[[git_syncs]]` tables. A
/// dedicated function rather than a `patch_config_toml` call because the
/// value being patched is a structured list, not a single scalar.
pub(crate) fn patch_git_syncs(
    path: &Path,
    mutate: impl FnOnce(&mut Vec<GitSyncConfig>),
) -> std::io::Result<()> {
    let mut table = read_config_toml(path)?;
    let mut list: Vec<GitSyncConfig> = match table.remove("git_syncs") {
        Some(value) => value.try_into().map_err(|e: toml::de::Error| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?,
        None => Vec::new(),
    };
    mutate(&mut list);
    let value = toml::Value::try_from(&list)
        .map_err(|e| std::io::Error::other(format!("failed to serialize git_syncs: {e}")))?;
    table.insert("git_syncs".to_string(), value);
    write_config_toml(path, &table)
}

/// Shared application state handed to every Axum handler.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub quadlet_dir: Arc<PathBuf>,
    pub systemd: Arc<Client>,
    /// Best-effort host-dependency checks -- see `crate::health`. Wraps its
    /// own `Arc`, refreshable in place (Settings' "System" card), so every
    /// clone of `AppState` shares one live snapshot rather than freezing
    /// whatever was true at startup.
    pub health: HealthCell,
    pub events: EventSender,
    /// The TOML file `Config::load` actually resolved and read (whether or
    /// not it existed yet) -- kept around so the Settings page can write
    /// back to the exact same file, rather than re-deriving the path (and
    /// potentially disagreeing with it) at request time.
    pub config_path: Arc<PathBuf>,
    /// Flips to `true` when a shutdown signal arrives. SSE handlers
    /// (`/events`, `/units/:file/logs/stream`) are otherwise infinite
    /// streams -- axum's graceful shutdown waits for in-flight requests to
    /// finish before exiting, and an open browser tab holding one of these
    /// connections would make that wait forever. Those handlers race their
    /// stream against this flag and end it once shutdown is signaled.
    pub shutdown: tokio::sync::watch::Receiver<bool>,
    /// Notified by the Settings page's "Restart" button. `main` treats it
    /// like a shutdown signal, then re-execs the binary instead of exiting
    /// -- the deployment-agnostic way to reload config that a systemd
    /// `restart` (needs a unit) or a plain exit (needs `Restart=`) don't
    /// cover.
    pub restart: Arc<tokio::sync::Notify>,
    /// Live supervisor for every configured git-synced group -- see
    /// `crate::quadlet::gitsync`. Unlike the rest of this struct's config,
    /// syncs are added/removed at runtime (no restart), so this is a handle
    /// to running background tasks, not just a config snapshot.
    pub git_sync: GitSyncManager,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_config_toml_preserves_unmanaged_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "auth_password_hash = \"$argon2id$abc\"\nbind_addr = \"127.0.0.1:8420\"\n",
        )
        .unwrap();

        patch_config_toml(
            &path,
            &[("bind_addr", toml::Value::String("0.0.0.0:9000".to_string()))],
        )
        .unwrap();

        let reparsed: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(reparsed["bind_addr"].as_str(), Some("0.0.0.0:9000"));
        assert_eq!(
            reparsed["auth_password_hash"].as_str(),
            Some("$argon2id$abc"),
            "a general settings save must not drop the password hash"
        );
    }

    #[test]
    fn patch_config_toml_creates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");

        patch_config_toml(
            &path,
            &[(
                "auth_password_hash",
                toml::Value::String("$argon2id$xyz".to_string()),
            )],
        )
        .unwrap();

        let reparsed: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            reparsed["auth_password_hash"].as_str(),
            Some("$argon2id$xyz")
        );
    }

    #[test]
    fn patch_git_syncs_appends_and_preserves_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "bind_addr = \"127.0.0.1:8420\"\n").unwrap();

        patch_git_syncs(&path, |list| {
            list.push(GitSyncConfig {
                group: "media".to_string(),
                remote: "https://example.invalid/repo.git".to_string(),
                branch: Some("main".to_string()),
                poll_interval_secs: 60,
            })
        })
        .unwrap();

        let reloaded = Config::load(Some(path.clone())).unwrap();
        assert_eq!(reloaded.git_syncs.len(), 1);
        assert_eq!(reloaded.git_syncs[0].group, "media");
        assert_eq!(reloaded.bind_addr.to_string(), "127.0.0.1:8420");
    }

    #[test]
    fn patch_git_syncs_can_remove_an_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        patch_git_syncs(&path, |list| {
            list.push(GitSyncConfig {
                group: "media".to_string(),
                remote: "https://example.invalid/repo.git".to_string(),
                branch: Some("main".to_string()),
                poll_interval_secs: 60,
            })
        })
        .unwrap();
        patch_git_syncs(&path, |list| list.retain(|c| c.group != "media")).unwrap();

        let reloaded = Config::load(Some(path)).unwrap();
        assert!(reloaded.git_syncs.is_empty());
    }
}
