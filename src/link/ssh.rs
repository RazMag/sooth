//! SSH links: for a machine the hub can reach over SSH but not on the
//! agent's port, the hub runs `ssh -N -L` to the agent's loopback port and
//! talks to it through that tunnel. The tunnel is only transport -- the
//! agent API on top is the same mutual TLS with pinned keys as a direct
//! link, so SSH adds a way in, not a different trust.
//!
//! - [`SshKey`]: the hub's own SSH key (`link/ssh_ed25519`), made with
//!   `ssh-keygen` the first time it's needed. Its public half goes in the
//!   machine's `~/.ssh/authorized_keys`.
//! - [`SshTarget`]: `ssh://user@host[:port]` (or plain `user@host`).
//! - [`Tunnel`]: one supervised `ssh` process, restarted with backoff when it
//!   exits, on a local port fixed for the tunnel's life.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

/// The port an agent listens on unless told otherwise.
pub const DEFAULT_AGENT_PORT: u16 = 8421;

/// The longest wait between two attempts to bring a tunnel back.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// The hub's SSH key.
#[derive(Debug, Clone)]
pub struct SshKey {
    pub path: PathBuf,
    /// The `authorized_keys` line: `ssh-ed25519 AAAA... sooth hub <name>`.
    pub public: String,
}

impl SshKey {
    /// Reads `ssh_ed25519` from `dir`, making it with `ssh-keygen` first if
    /// it isn't there. Fails when `ssh-keygen` isn't installed.
    pub fn load_or_create(dir: &Path, comment: &str) -> anyhow::Result<Self> {
        let path = dir.join("ssh_ed25519");
        let public_path = dir.join("ssh_ed25519.pub");
        if !path.exists() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            let status = std::process::Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
                .arg(&path)
                .stdin(Stdio::null())
                .status()
                .context("running ssh-keygen (is OpenSSH installed?)")?;
            anyhow::ensure!(status.success(), "ssh-keygen failed ({status})");
            tracing::info!(path = %path.display(), "made this hub's SSH key");
        }
        let public = std::fs::read_to_string(&public_path)
            .with_context(|| format!("reading {}", public_path.display()))?
            .trim()
            .to_string();
        Ok(Self { path, public })
    }
}

/// Where to SSH to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl SshTarget {
    /// `ssh://user@host:2222`, `user@host`, `host`, with `[v6]` for an IPv6
    /// host. Anything that could reach `ssh` as an option (a leading `-`)
    /// or as more than one argument is refused.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix("ssh://").unwrap_or(s).trim_end_matches('/');
        let (user, rest) = match s.rsplit_once('@') {
            Some((u, r)) => (Some(u.to_string()), r),
            None => (None, s),
        };
        let (host, port) = if let Some(v6) = rest.strip_prefix('[') {
            let (host, after) = v6.split_once(']')?;
            let port = match after.strip_prefix(':') {
                Some(p) => Some(p.parse().ok()?),
                None if after.is_empty() => None,
                None => return None,
            };
            (host.to_string(), port)
        } else {
            match rest.split_once(':') {
                Some((h, p)) => (h.to_string(), Some(p.parse().ok()?)),
                None => (rest.to_string(), None),
            }
        };
        let fine = |part: &str| {
            !part.is_empty()
                && !part.starts_with('-')
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-:%".contains(c))
        };
        if !fine(&host) || user.as_deref().is_some_and(|u| !fine(u) || u.contains(':')) {
            return None;
        }
        Some(Self { user, host, port })
    }

    /// The `ssh://` form, as saved and shown.
    pub fn address(&self) -> String {
        let user = self
            .user
            .as_deref()
            .map(|u| format!("{u}@"))
            .unwrap_or_default();
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(p) => format!("ssh://{user}{host}:{p}"),
            None => format!("ssh://{user}{host}"),
        }
    }

    /// `user@host` as `ssh` takes it (the port goes in `-p`).
    fn destination(&self) -> String {
        match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        }
    }
}

/// Whether `address` is an SSH link: `ssh://...`, or a bare `user@host`.
pub fn is_ssh(address: &str) -> bool {
    let address = address.trim();
    address.starts_with("ssh://") || (!address.contains("://") && address.contains('@'))
}

/// How a tunnel is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelState {
    Starting,
    Up,
    /// `ssh` exited; the last thing it said, and it's retried shortly.
    Down(String),
}

struct Shared {
    state: TunnelState,
    /// The running `ssh`, kept here so [`Tunnel::close`] can kill it at once.
    child: Option<Child>,
    closed: bool,
}

/// One supervised `ssh -N -L 127.0.0.1:<local>:127.0.0.1:<agent_port>`.
/// Dropping it (or [`Tunnel::close`]) kills the process.
pub struct Tunnel {
    local_port: u16,
    target: SshTarget,
    shared: Arc<Mutex<Shared>>,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel")
            .field("target", &self.target.address())
            .field("local_port", &self.local_port)
            .finish()
    }
}

/// What `ssh` needs from the hub's link folder.
#[derive(Debug, Clone)]
pub struct SshSetup {
    pub key: SshKey,
    /// The hub's own `known_hosts` for these links (`link/known_hosts`):
    /// a host is trusted on first use, and refused if its key later changes.
    pub known_hosts: PathBuf,
}

impl Tunnel {
    /// Starts the tunnel; it keeps itself up until closed.
    pub fn start(setup: &SshSetup, target: SshTarget, agent_port: u16) -> std::io::Result<Self> {
        // A local port picked once, so the agent's URL stays the same across
        // restarts of `ssh`.
        let local_port = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
            .local_addr()?
            .port();
        let shared = Arc::new(Mutex::new(Shared {
            state: TunnelState::Starting,
            child: None,
            closed: false,
        }));
        let task = tokio::spawn(supervise(
            setup.clone(),
            target.clone(),
            local_port,
            agent_port,
            shared.clone(),
        ));
        Ok(Self {
            local_port,
            target,
            shared,
            task,
        })
    }

    /// The agent's address through the tunnel.
    pub fn url(&self) -> String {
        format!("https://127.0.0.1:{}", self.local_port)
    }

    pub fn state(&self) -> TunnelState {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .state
            .clone()
    }

    /// Waits until the tunnel is up, or says why it isn't by `timeout`.
    pub async fn ready(&self, timeout: Duration) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match self.state() {
                TunnelState::Up => return Ok(()),
                TunnelState::Down(reason) => return Err(reason),
                TunnelState::Starting => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "no SSH connection to {} within {} seconds",
                    self.target.address(),
                    timeout.as_secs()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The running `ssh`'s process id, for tests that pull the plug.
    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .child
            .as_ref()
            .and_then(Child::id)
    }

    /// Kills `ssh` now and stops restarting it. Safe to call more than once.
    pub fn close(&self) {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        shared.closed = true;
        if let Some(child) = shared.child.as_mut() {
            let _ = child.start_kill();
        }
        drop(shared);
        self.task.abort();
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.close();
    }
}

fn ssh_command(setup: &SshSetup, target: &SshTarget, local_port: u16, agent_port: u16) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.arg("-N")
        .arg("-T")
        .arg("-i")
        .arg(&setup.key.path)
        .args(["-o", "IdentitiesOnly=yes"])
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ExitOnForwardFailure=yes"])
        .args(["-o", "ConnectTimeout=10"])
        .args(["-o", "ServerAliveInterval=15"])
        .args(["-o", "ServerAliveCountMax=3"])
        .args(["-o", "StrictHostKeyChecking=accept-new"])
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            setup.known_hosts.display()
        ))
        .arg("-L")
        .arg(format!("127.0.0.1:{local_port}:127.0.0.1:{agent_port}"));
    if let Some(port) = target.port {
        cmd.arg("-p").arg(port.to_string());
    }
    // `--` so the destination can never be read as an option.
    cmd.arg("--").arg(target.destination());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

fn set(shared: &Mutex<Shared>, state: TunnelState) {
    shared.lock().unwrap_or_else(|e| e.into_inner()).state = state;
}

async fn supervise(
    setup: SshSetup,
    target: SshTarget,
    local_port: u16,
    agent_port: u16,
    shared: Arc<Mutex<Shared>>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let reason = run_once(
            &setup,
            &target,
            local_port,
            agent_port,
            &shared,
            &mut backoff,
        )
        .await;
        if shared.lock().unwrap_or_else(|e| e.into_inner()).closed {
            return;
        }
        tracing::warn!(target = %target.address(), %reason, "SSH tunnel down; retrying");
        set(&shared, TunnelState::Down(reason));
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Runs `ssh` until it exits, and returns why it did.
async fn run_once(
    setup: &SshSetup,
    target: &SshTarget,
    local_port: u16,
    agent_port: u16,
    shared: &Mutex<Shared>,
    backoff: &mut Duration,
) -> String {
    let mut child = match ssh_command(setup, target, local_port, agent_port).spawn() {
        Ok(c) => c,
        Err(e) => return format!("couldn't run ssh: {e}"),
    };
    let last_line = Arc::new(Mutex::new(String::new()));
    if let Some(stderr) = child.stderr.take() {
        let last_line = last_line.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim().to_string();
                // ssh's notice about adding a new host isn't a failure.
                if !line.is_empty() && !line.starts_with("Warning: Permanently added") {
                    *last_line.lock().unwrap_or_else(|e| e.into_inner()) = line;
                }
            }
        });
    }
    {
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        if s.closed {
            let _ = child.start_kill();
            return "closed".into();
        }
        s.child = Some(child);
    }
    let local = SocketAddr::from((Ipv4Addr::LOCALHOST, local_port));
    let mut up = false;
    let status = loop {
        let exited = {
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            match s.child.as_mut().map(Child::try_wait) {
                Some(Ok(Some(status))) => Some(Ok(status)),
                Some(Err(e)) => Some(Err(e)),
                Some(Ok(None)) => None,
                None => Some(Err(std::io::Error::other("closed"))),
            }
        };
        if let Some(status) = exited {
            break status;
        }
        // The local end only listens once ssh is logged in and forwarding.
        if !up && tokio::net::TcpStream::connect(local).await.is_ok() {
            up = true;
            *backoff = Duration::from_secs(1);
            set(shared, TunnelState::Up);
            tracing::info!(target = %target.address(), local_port, "SSH tunnel up");
        }
        tokio::time::sleep(Duration::from_millis(if up { 500 } else { 100 })).await;
    };
    shared.lock().unwrap_or_else(|e| e.into_inner()).child = None;
    // Give the stderr reader a moment to catch ssh's last words.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let said = last_line.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match (said.is_empty(), status) {
        (false, _) => said,
        (true, Ok(status)) => format!("ssh exited ({status})"),
        (true, Err(e)) => format!("ssh: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_parse_and_print_back() {
        let t = SshTarget::parse("ssh://pi@10.0.0.5:2222").unwrap();
        assert_eq!(t.user.as_deref(), Some("pi"));
        assert_eq!(t.host, "10.0.0.5");
        assert_eq!(t.port, Some(2222));
        assert_eq!(t.address(), "ssh://pi@10.0.0.5:2222");
        assert_eq!(t.destination(), "pi@10.0.0.5");

        let t = SshTarget::parse("nas.local").unwrap();
        assert_eq!((t.user, t.port), (None, None));
        assert_eq!(
            SshTarget::parse("pi@nas").unwrap().address(),
            "ssh://pi@nas"
        );

        let t = SshTarget::parse("ssh://root@[fd00::5]:22").unwrap();
        assert_eq!(t.host, "fd00::5");
        assert_eq!(t.address(), "ssh://root@[fd00::5]:22");

        for bad in [
            "",
            "ssh://",
            "-oProxyCommand=x",
            "pi@-host",
            "pi@host:port",
            "pi@ho st",
            "p i@host",
            "pi@host;rm",
            "ssh://[fd00::5",
        ] {
            assert_eq!(SshTarget::parse(bad), None, "{bad}");
        }
        assert!(is_ssh(" ssh://pi@nas"));
        assert!(is_ssh("pi@nas"));
        assert!(!is_ssh("https://nas:8421"));
    }

    #[test]
    fn the_command_forwards_to_the_agent_and_cant_take_options_from_the_target() {
        let setup = SshSetup {
            key: SshKey {
                path: "/cfg/link/ssh_ed25519".into(),
                public: String::new(),
            },
            known_hosts: "/cfg/link/known_hosts".into(),
        };
        let target = SshTarget::parse("pi@nas:2222").unwrap();
        let cmd = ssh_command(&setup, &target, 40001, 8421);
        let args: Vec<String> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        assert!(joined.contains("-L 127.0.0.1:40001:127.0.0.1:8421"));
        assert!(joined.contains("-i /cfg/link/ssh_ed25519"));
        assert!(joined.contains("UserKnownHostsFile=/cfg/link/known_hosts"));
        assert!(joined.contains("BatchMode=yes"));
        assert!(joined.contains("-p 2222"));
        assert!(joined.ends_with("-- pi@nas"));
    }
}
