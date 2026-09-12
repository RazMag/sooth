use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};

use crate::events::EventSender;
use crate::health::HealthCell;
use crate::quadlet::QuadletError;
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
}
