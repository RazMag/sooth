//! The hub's machines: every agent it has paired with, kept in
//! `machines.toml` beside the config file, with this hub's own link key in
//! `link/` next to it. Copying that config folder to another machine moves
//! the hub with all its pairings.
//!
//! The fleet also remembers, in memory, how each machine last answered
//! ([`Seen`]) and the last unit listing each page got from it, so a machine
//! that drops off still shows what it was running, marked as such
//! ([`Listing::Stale`]). [`monitor`] re-checks every machine in the
//! background and tells open pages when one comes or goes.

pub mod monitor;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::events::{DashboardEvent, EventSender};
use crate::link::ssh::{
    self, DEFAULT_AGENT_PORT, SshKey, SshSetup, SshTarget, Tunnel, TunnelState,
};
use crate::link::trust::{Permission, now_secs};
use crate::link::{API_VERSION, Fingerprint, Identity};
use crate::machine::api::Hello;
use crate::machine::remote::{self, RemoteMachine};
use crate::machine::{Facts, Machine, UnitListing};
use crate::quadlet::{QuadletError, UnitKind};

/// How long the hub waits for a machine before calling it offline.
pub const REACH_TIMEOUT: Duration = Duration::from_secs(4);

/// How a paired machine answered when last asked.
#[derive(Debug, Clone)]
pub enum Reach {
    Online { facts: Facts, millis: u128 },
    Offline { reason: String },
}

impl Reach {
    pub fn is_online(&self) -> bool {
        matches!(self, Self::Online { .. })
    }
}

/// The last word from a machine.
#[derive(Debug, Clone)]
pub struct Seen {
    pub reach: Reach,
    /// When it last answered, if it ever has since the hub started.
    pub online_at: Option<u64>,
}

/// A machine's units for one page: fresh, or the last ones it gave if it
/// doesn't answer now.
pub enum Listing {
    Live(UnitListing),
    Stale {
        listing: UnitListing,
        as_of: u64,
        reason: String,
    },
    Unreachable {
        reason: String,
    },
}

impl Listing {
    /// The units to draw, fresh or not (none when it never answered).
    pub fn units(&self) -> Option<&UnitListing> {
        match self {
            Self::Live(l) | Self::Stale { listing: l, .. } => Some(l),
            Self::Unreachable { .. } => None,
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }
}

/// One paired machine as saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineEntry {
    /// URL-safe, unique among the hub's machines, made from the name.
    pub id: String,
    pub name: String,
    /// `https://host:port` of its agent, or `ssh://user@host[:port]` for a
    /// machine reached through an SSH tunnel (see `link::ssh`).
    pub address: String,
    /// The agent's port on that machine, for an SSH link (8421 if unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_port: Option<u16>,
    pub fingerprint: Fingerprint,
    /// What the agent granted at pairing.
    pub permission: Permission,
    pub added_at: u64,
}

impl MachineEntry {
    /// The host its address names (no brackets around an IPv6 one) -- where
    /// a browser would reach the services it publishes.
    pub fn host(&self) -> Option<String> {
        let host = if crate::link::ssh::is_ssh(&self.address) {
            crate::link::ssh::SshTarget::parse(&self.address)?.host
        } else {
            reqwest::Url::parse(&self.address)
                .ok()?
                .host_str()?
                .to_string()
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        (!host.is_empty()).then(|| host.to_string())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct MachinesFile {
    #[serde(default, rename = "machine")]
    machines: Vec<MachineEntry>,
}

/// A paired machine and a client for it.
#[derive(Clone)]
pub struct Linked {
    pub entry: MachineEntry,
    pub remote: RemoteMachine,
    /// The SSH tunnel the client goes through, for an SSH link.
    pub tunnel: Option<Arc<Tunnel>>,
}

impl Linked {
    /// The machine, for code that works on any [`Machine`].
    pub fn machine(&self) -> Machine {
        Machine::Remote(self.remote.clone())
    }

    /// Why a call failed, in words: what `ssh` last said when the tunnel
    /// is down, since that's the real cause, otherwise the error itself.
    fn reason(&self, e: &AppError) -> String {
        match self.tunnel.as_ref().map(|t| t.state()) {
            Some(TunnelState::Down(why)) => format!("SSH: {why}"),
            Some(TunnelState::Starting) => "SSH: still connecting".into(),
            _ => e.user_message(),
        }
    }

    /// How the hub reaches it, for the Machines page.
    pub fn via(&self) -> &'static str {
        if self.tunnel.is_some() {
            "SSH"
        } else {
            "HTTPS"
        }
    }
}

/// How long adding a machine over SSH waits for the tunnel.
const SSH_READY: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct Fleet {
    inner: Arc<Inner>,
}

struct Inner {
    identity: Identity,
    path: PathBuf,
    /// `link/` beside the config file.
    link_dir: PathBuf,
    /// The hub's SSH key and known hosts, made the first time they're needed.
    ssh: Mutex<Option<SshSetup>>,
    /// What this hub calls itself to agents (its hostname).
    hub_name: String,
    machines: RwLock<Vec<Linked>>,
    seen: RwLock<HashMap<String, Seen>>,
    /// The last listing per machine and set of kinds.
    listings: Mutex<HashMap<ListingKey, (UnitListing, u64)>>,
    events: OnceLock<EventSender>,
}

fn invalid(msg: impl Into<String>) -> AppError {
    QuadletError::Validation(msg.into()).into()
}

/// `Pi in the garage` -> `pi-in-the-garage`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "machine".into()
    } else {
        out
    }
}

pub fn valid_name(name: &str) -> Result<&str, AppError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
        return Err(invalid("give the machine a name of 1 to 40 characters"));
    }
    Ok(name)
}

impl Fleet {
    /// This hub's key (made on first start) and its saved machines, from the
    /// folder holding `config_path`.
    pub fn load(config_path: &Path, hub_name: &str) -> anyhow::Result<Self> {
        let link_dir = crate::link::dir_beside(config_path);
        let identity = Identity::load_or_create(&link_dir)?;
        let path = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("machines.toml");
        let entries = match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str::<MachinesFile>(&text)
                    .map_err(|e| anyhow::anyhow!("{} is not valid: {e}", path.display()))?
                    .machines
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let fleet = Self {
            inner: Arc::new(Inner {
                identity,
                path,
                link_dir,
                ssh: Mutex::new(None),
                hub_name: hub_name.to_string(),
                machines: RwLock::new(Vec::new()),
                seen: RwLock::new(HashMap::new()),
                listings: Mutex::new(HashMap::new()),
                events: OnceLock::new(),
            }),
        };
        let mut machines = Vec::new();
        for entry in entries {
            match fleet.connect(entry.clone()) {
                Ok(linked) => machines.push(linked),
                Err(e) => {
                    tracing::warn!(machine = %entry.name, error = %e, "skipping a saved machine")
                }
            }
        }
        *fleet
            .inner
            .machines
            .write()
            .unwrap_or_else(|e| e.into_inner()) = machines;
        Ok(fleet)
    }

    /// The hub's SSH key and known-hosts file, making the key with
    /// `ssh-keygen` the first time.
    pub fn ssh_setup(&self) -> Result<SshSetup, AppError> {
        let mut slot = self.inner.ssh.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(setup) = slot.as_ref() {
            return Ok(setup.clone());
        }
        let key = SshKey::load_or_create(
            &self.inner.link_dir,
            &format!("sooth-hub@{}", self.inner.hub_name),
        )
        .map_err(|e| AppError::Remote(format!("couldn't make this hub's SSH key: {e:#}")))?;
        let setup = SshSetup {
            key,
            known_hosts: self.inner.link_dir.join("known_hosts"),
        };
        *slot = Some(setup.clone());
        Ok(setup)
    }

    /// Starts an SSH tunnel to the agent behind `target`.
    fn tunnel(&self, target: SshTarget, agent_port: u16) -> Result<Tunnel, AppError> {
        Tunnel::start(&self.ssh_setup()?, target, agent_port)
            .map_err(|e| AppError::Remote(format!("couldn't start ssh: {e}")))
    }

    /// A client for a saved machine, over its SSH tunnel when it has one.
    fn connect(&self, entry: MachineEntry) -> Result<Linked, AppError> {
        let (base, tunnel) = if ssh::is_ssh(&entry.address) {
            let target = SshTarget::parse(&entry.address)
                .ok_or_else(|| invalid(format!("'{}' isn't an SSH address", entry.address)))?;
            let tunnel = self.tunnel(target, entry.agent_port.unwrap_or(DEFAULT_AGENT_PORT))?;
            (tunnel.url(), Some(Arc::new(tunnel)))
        } else {
            (entry.address.clone(), None)
        };
        let remote = RemoteMachine::new(&base, &self.inner.identity, entry.fingerprint)?;
        Ok(Linked {
            entry,
            remote,
            tunnel,
        })
    }

    /// Where the agent at `address` is reached from here: the address
    /// itself, or a fresh SSH tunnel's local end (kept open by the returned
    /// tunnel). Also `address` as it's saved.
    async fn reach(
        &self,
        address: &str,
        agent_port: Option<u16>,
    ) -> Result<(String, String, Option<Tunnel>), AppError> {
        let address = address.trim();
        if !ssh::is_ssh(address) {
            return Ok((address.to_string(), address.to_string(), None));
        }
        let target = SshTarget::parse(address).ok_or_else(|| {
            invalid("write an SSH address as ssh://user@host, or ssh://user@host:port")
        })?;
        let saved = target.address();
        let tunnel = self.tunnel(target, agent_port.unwrap_or(DEFAULT_AGENT_PORT))?;
        tunnel.ready(SSH_READY).await.map_err(|why| {
            AppError::Remote(format!(
                "couldn't open an SSH tunnel to {saved}: {why}. Check that this hub's \
                 SSH key is in that user's ~/.ssh/authorized_keys."
            ))
        })?;
        Ok((tunnel.url(), saved, Some(tunnel)))
    }

    /// Kills every SSH tunnel at once -- before the hub re-executes itself,
    /// which would otherwise leave them running.
    pub fn close_tunnels(&self) {
        for m in self.machines() {
            if let Some(t) = &m.tunnel {
                t.close();
            }
        }
    }

    /// Where to announce a machine coming online or dropping off
    /// ([`DashboardEvent::MachinesChanged`]).
    pub fn notify_on(&self, events: EventSender) {
        let _ = self.inner.events.set(events);
    }

    fn announce(&self) {
        if let Some(events) = self.inner.events.get() {
            let _ = events.send(DashboardEvent::MachinesChanged);
        }
    }

    /// How the machine `id` last answered, if it's been asked.
    pub fn seen(&self, id: &str) -> Option<Seen> {
        self.inner
            .seen
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// Notes how `id` answered. An answer of another sort than the last one
    /// (it came back, or dropped off) is announced.
    fn record(&self, id: &str, reach: Reach) {
        let now = now_secs();
        let flipped = {
            let mut seen = self.inner.seen.write().unwrap_or_else(|e| e.into_inner());
            let before = seen.get(id).cloned();
            let online_at = if reach.is_online() {
                Some(now)
            } else {
                before.as_ref().and_then(|b| b.online_at)
            };
            let flipped = before
                .as_ref()
                .is_none_or(|b| b.reach.is_online() != reach.is_online());
            seen.insert(id.to_string(), Seen { reach, online_at });
            flipped
        };
        if flipped {
            self.announce();
        }
    }

    /// Asks one machine about itself, and notes the answer.
    pub async fn check(&self, linked: &Linked) -> Reach {
        let started = Instant::now();
        let reach = match tokio::time::timeout(REACH_TIMEOUT, linked.remote.facts()).await {
            Ok(Ok(facts)) => Reach::Online {
                facts,
                millis: started.elapsed().as_millis(),
            },
            Ok(Err(e)) => Reach::Offline {
                reason: linked.reason(&e),
            },
            Err(_) => Reach::Offline {
                reason: no_answer(),
            },
        };
        self.record(&linked.entry.id, reach.clone());
        reach
    }

    /// [`Fleet::check`] on every machine at once.
    pub async fn check_all(&self) -> Vec<(Linked, Reach)> {
        join_all(self.machines().into_iter().map(|m| async move {
            let reach = self.check(&m).await;
            (m, reach)
        }))
        .await
    }

    /// The last units of `kinds` the machine `id` gave, without asking it.
    pub fn cached(&self, id: &str, kinds: &[UnitKind]) -> Option<UnitListing> {
        self.inner
            .listings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&listing_key(id, kinds))
            .map(|(l, _)| l.clone())
    }

    /// The machine's units of `kinds`, or the last ones it gave if it doesn't
    /// answer within [`REACH_TIMEOUT`].
    pub async fn listing(&self, linked: &Linked, kinds: &[UnitKind]) -> Listing {
        let id = &linked.entry.id;
        let key = listing_key(id, kinds);
        let reason = match tokio::time::timeout(REACH_TIMEOUT, linked.machine().units(kinds)).await
        {
            Ok(Ok(listing)) => {
                self.inner
                    .listings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, (listing.clone(), now_secs()));
                // Units came back, so it's up; keep the facts it last gave.
                if self.seen(id).is_none_or(|s| !s.reach.is_online()) {
                    let linked = linked.clone();
                    let fleet = self.clone();
                    tokio::spawn(async move { fleet.check(&linked).await });
                }
                return Listing::Live(listing);
            }
            Ok(Err(e)) => linked.reason(&e),
            Err(_) => no_answer(),
        };
        self.record(
            id,
            Reach::Offline {
                reason: reason.clone(),
            },
        );
        match self
            .inner
            .listings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
        {
            Some((listing, as_of)) => Listing::Stale {
                listing,
                as_of,
                reason,
            },
            None => Listing::Unreachable { reason },
        }
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.inner.identity.fingerprint
    }

    pub fn machines(&self) -> Vec<Linked> {
        self.inner
            .machines
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn get(&self, id: &str) -> Option<Linked> {
        self.machines().into_iter().find(|m| m.entry.id == id)
    }

    fn save(&self, machines: &[Linked]) -> Result<(), AppError> {
        let file = MachinesFile {
            machines: machines.iter().map(|m| m.entry.clone()).collect(),
        };
        let text = toml::to_string_pretty(&file).map_err(|e| anyhow::anyhow!(e))?;
        crate::link::write_private(&self.inner.path, &text)?;
        Ok(())
    }

    /// The first look at a machine being added: its key's fingerprint and
    /// what it says about itself. Refuses an agent on another API version.
    /// `agent_port` is the agent's port behind an SSH address.
    pub async fn probe(
        &self,
        address: &str,
        agent_port: Option<u16>,
    ) -> Result<(Fingerprint, Hello), AppError> {
        let (url, saved, _tunnel) = self.reach(address, agent_port).await?;
        if self.machines().iter().any(|m| m.entry.address == saved) {
            return Err(invalid(format!("{saved} is already one of your machines")));
        }
        let (fingerprint, hello) = remote::probe(&url, &self.inner.identity).await?;
        if hello.api_version != API_VERSION {
            return Err(invalid(format!(
                "{} runs sooth {} (agent API v{}); this hub speaks v{API_VERSION}. \
                 Update the older one first.",
                hello.hostname, hello.sooth_version, hello.api_version
            )));
        }
        if fingerprint == self.fingerprint() {
            return Err(invalid("that address is this hub itself"));
        }
        Ok((fingerprint, hello))
    }

    /// Pairs with the agent at `address` (key `fingerprint`, confirmed by the
    /// person) using its code, and saves it as `name`.
    pub async fn add(
        &self,
        address: &str,
        agent_port: Option<u16>,
        fingerprint: Fingerprint,
        code: &str,
        name: &str,
    ) -> Result<MachineEntry, AppError> {
        let name = valid_name(name)?;
        let (url, saved, tunnel) = self.reach(address, agent_port).await?;
        let paired = remote::pair(
            &url,
            &self.inner.identity,
            fingerprint,
            code,
            &self.inner.hub_name,
        )
        .await?;
        let remote = RemoteMachine::new(&url, &self.inner.identity, fingerprint)?;
        let agent_port = tunnel
            .as_ref()
            .map(|_| agent_port.unwrap_or(DEFAULT_AGENT_PORT));

        let mut machines = self
            .inner
            .machines
            .write()
            .unwrap_or_else(|e| e.into_inner());
        let base = slug(name);
        let mut id = base.clone();
        let mut n = 2;
        while machines.iter().any(|m| m.entry.id == id) || id == "local" {
            id = format!("{base}-{n}");
            n += 1;
        }
        let entry = MachineEntry {
            id,
            name: name.to_string(),
            address: saved,
            agent_port,
            fingerprint,
            permission: paired.permission,
            added_at: now_secs(),
        };
        machines.retain(|m| m.entry.fingerprint != fingerprint);
        machines.push(Linked {
            entry: entry.clone(),
            remote,
            tunnel: tunnel.map(Arc::new),
        });
        self.save(&machines)?;
        drop(machines);
        self.announce();
        tracing::info!(machine = %entry.name, address = %entry.address, %fingerprint, "machine paired");
        Ok(entry)
    }

    /// Unpairs a machine: asks its agent to forget this hub (best effort --
    /// an offline machine is still removed here) and drops it from the list.
    /// Returns whether the agent confirmed.
    pub async fn remove(&self, id: &str) -> Result<bool, AppError> {
        let linked = self
            .get(id)
            .ok_or_else(|| AppError::NotFound(id.to_string()))?;
        let confirmed = match linked.remote.unpair().await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(machine = %linked.entry.name, error = %e, "the agent didn't confirm unpairing");
                false
            }
        };
        let mut machines = self
            .inner
            .machines
            .write()
            .unwrap_or_else(|e| e.into_inner());
        machines.retain(|m| m.entry.id != id);
        self.save(&machines)?;
        drop(machines);
        if let Some(t) = &linked.tunnel {
            t.close();
        }
        self.forget(id);
        tracing::info!(machine = %linked.entry.name, confirmed, "machine unpaired");
        Ok(confirmed)
    }

    fn forget(&self, id: &str) {
        self.inner
            .seen
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        self.inner
            .listings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(m, _), _| m != id);
        self.announce();
    }

    /// Renames a machine (its id stays, so links keep working).
    pub fn rename(&self, id: &str, name: &str) -> Result<(), AppError> {
        let name = valid_name(name)?;
        let mut machines = self
            .inner
            .machines
            .write()
            .unwrap_or_else(|e| e.into_inner());
        let m = machines
            .iter_mut()
            .find(|m| m.entry.id == id)
            .ok_or_else(|| AppError::NotFound(id.to_string()))?;
        m.entry.name = name.to_string();
        self.save(&machines)?;
        drop(machines);
        self.announce();
        Ok(())
    }
}

/// A machine id and a set of kinds.
type ListingKey = (String, Vec<String>);

fn listing_key(id: &str, kinds: &[UnitKind]) -> ListingKey {
    (
        id.to_string(),
        kinds.iter().map(|k| format!("{k:?}")).collect(),
    )
}

fn no_answer() -> String {
    format!("no answer within {} seconds", REACH_TIMEOUT.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::remote::tests::{fake, spawn_agent};

    #[test]
    fn a_machines_host_comes_from_its_address() {
        let entry = |address: &str| MachineEntry {
            id: "pi".into(),
            name: "Pi".into(),
            address: address.into(),
            agent_port: None,
            fingerprint: Identity::generate().unwrap().fingerprint,
            permission: Permission::Full,
            added_at: 0,
        };
        let host = |a: &str| entry(a).host();
        assert_eq!(host("https://10.0.0.5:8421").as_deref(), Some("10.0.0.5"));
        assert_eq!(host("ssh://me@pi.lan:2222").as_deref(), Some("pi.lan"));
        assert_eq!(host("me@pi.lan").as_deref(), Some("pi.lan"));
        assert_eq!(host("ssh://me@[fd00::5]").as_deref(), Some("fd00::5"));
        assert_eq!(host("https://[fd00::5]:8421").as_deref(), Some("fd00::5"));
    }

    #[tokio::test]
    async fn a_machine_that_stops_answering_shows_its_last_units() {
        let dir = tempfile::tempdir().unwrap();
        let fleet = Fleet::load(&dir.path().join("config.toml"), "hub01").unwrap();
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        fleet.notify_on(tx);
        let agent = spawn_agent(fake()).await;
        let code = agent.hubs.issue_code(Permission::Full).unwrap();
        let (fp, _) = fleet.probe(&agent.url, None).await.unwrap();
        fleet.add(&agent.url, None, fp, &code, "Pi").await.unwrap();
        let linked = fleet.get("pi").unwrap();
        let kinds = [UnitKind::Container];

        match fleet.listing(&linked, &kinds).await {
            Listing::Live((units, all)) => {
                assert_eq!(units.len(), 1);
                assert_eq!(all.len(), 2);
            }
            _ => panic!("expected a live listing"),
        }
        assert!(fleet.check(&linked).await.is_online());
        assert!(fleet.seen("pi").unwrap().reach.is_online());

        // The agent forgets this hub, so it refuses it from now on.
        agent.hubs.remove(&fleet.fingerprint()).unwrap();
        match fleet.listing(&linked, &kinds).await {
            Listing::Stale {
                listing: (units, _),
                reason,
                ..
            } => {
                assert_eq!(units[0].0.file_name, "web.container");
                assert!(!reason.is_empty());
            }
            _ => panic!("expected the last listing, marked stale"),
        }
        let seen = fleet.seen("pi").unwrap();
        assert!(!seen.reach.is_online());
        assert!(seen.online_at.is_some(), "it did answer once");
        assert!(fleet.cached("pi", &kinds).is_some());
        // Kinds it never listed have nothing to fall back on.
        assert!(matches!(
            fleet.listing(&linked, &[UnitKind::Volume]).await,
            Listing::Unreachable { .. }
        ));

        // Pairing, coming online and dropping off were all announced.
        let mut announced = 0;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, DashboardEvent::MachinesChanged) {
                announced += 1;
            }
        }
        assert!(announced >= 3, "announced {announced} times");

        // Unpairing forgets what it last said.
        fleet.remove("pi").await.unwrap();
        assert!(fleet.seen("pi").is_none());
        assert!(fleet.cached("pi", &kinds).is_none());
    }

    /// A throwaway `sshd` on a loopback port that lets in `key` for the
    /// current user, or `None` when OpenSSH's server isn't installed.
    fn spawn_sshd(dir: &Path, key: &str) -> Option<(std::process::Child, u16)> {
        let sshd = ["/usr/sbin/sshd", "/usr/bin/sshd", "/sbin/sshd"]
            .into_iter()
            .find(|p| Path::new(p).exists())?;
        let host_key = dir.join("host_ed25519");
        let made = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&host_key)
            .status()
            .ok()?;
        assert!(made.success());
        let authorized = dir.join("authorized_keys");
        std::fs::write(&authorized, format!("{key}\n")).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = dir.join("sshd_config");
        std::fs::write(
            &config,
            format!(
                "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\n\
                 PasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\n\
                 StrictModes no\nPidFile {}\nAllowTcpForwarding yes\n",
                host_key.display(),
                authorized.display(),
                dir.join("sshd.pid").display()
            ),
        )
        .unwrap();
        let mut child = std::process::Command::new(sshd)
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        // Wait for it to listen; one that can't start here (e.g. as root
        // without /run/sshd) means the test is skipped.
        for _ in 0..50 {
            if let Ok(Some(_)) = child.try_wait() {
                return None;
            }
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Some((child, port));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        None
    }

    #[tokio::test]
    async fn a_machine_is_added_and_reached_through_an_ssh_tunnel() {
        let dir = tempfile::tempdir().unwrap();
        let fleet = Fleet::load(&dir.path().join("config.toml"), "hub01").unwrap();
        let Ok(setup) = fleet.ssh_setup() else {
            eprintln!("skipped: no ssh-keygen");
            return;
        };
        let sshd_dir = tempfile::tempdir().unwrap();
        let Some((mut sshd, ssh_port)) = spawn_sshd(sshd_dir.path(), &setup.key.public) else {
            eprintln!("skipped: no sshd");
            return;
        };
        let user = std::process::Command::new("id")
            .arg("-un")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap();
        let agent = spawn_agent(fake()).await;
        let agent_port: u16 = agent.url.rsplit(':').next().unwrap().parse().unwrap();
        let address = format!("{user}@127.0.0.1:{ssh_port}");

        let (fp, hello) = fleet.probe(&address, Some(agent_port)).await.unwrap();
        assert_eq!(fp, agent.identity.fingerprint);
        assert_eq!(hello.hostname, "fake01");

        let code = agent.hubs.issue_code(Permission::Full).unwrap();
        let entry = fleet
            .add(&address, Some(agent_port), fp, &code, "Tunnelled")
            .await
            .unwrap();
        assert_eq!(entry.address, format!("ssh://{user}@127.0.0.1:{ssh_port}"));
        assert_eq!(entry.agent_port, Some(agent_port));
        let linked = fleet.get("tunnelled").unwrap();
        assert_eq!(linked.via(), "SSH");
        assert!(linked.machine().units(&[UnitKind::Container]).await.is_ok());

        // A restart brings the tunnel back from machines.toml.
        let reloaded = Fleet::load(&dir.path().join("config.toml"), "hub01").unwrap();
        let again = reloaded.get("tunnelled").unwrap();
        again
            .tunnel
            .as_ref()
            .unwrap()
            .ready(Duration::from_secs(10))
            .await
            .unwrap();
        assert!(reloaded.check(&again).await.is_online());

        // sshd goes away, then the connection drops: the tunnel can't come
        // back, and the reason is what ssh says.
        let _ = sshd.kill();
        let _ = sshd.wait();
        let tunnel = again.tunnel.as_ref().unwrap();
        let pid = tunnel.pid().expect("ssh is running");
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
        for _ in 0..100 {
            if matches!(tunnel.state(), TunnelState::Down(_)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            matches!(tunnel.state(), TunnelState::Down(_)),
            "{:?}",
            tunnel.state()
        );
        match reloaded.check(&again).await {
            Reach::Offline { reason } => assert!(reason.starts_with("SSH: "), "{reason}"),
            Reach::Online { .. } => panic!("still online without sshd"),
        }
        fleet.close_tunnels();
        reloaded.close_tunnels();
    }

    #[test]
    fn slugs_are_url_safe() {
        assert_eq!(slug("Pi in the garage"), "pi-in-the-garage");
        assert_eq!(slug("  nas01 "), "nas01");
        assert_eq!(slug("***"), "machine");
        assert!(valid_name("").is_err());
        assert!(valid_name(&"x".repeat(41)).is_err());
    }

    #[tokio::test]
    async fn a_hub_adds_renames_and_removes_a_machine_and_remembers_it() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        let fleet = Fleet::load(&config, "hub01").unwrap();
        let agent = spawn_agent(fake()).await;
        let code = agent.hubs.issue_code(Permission::Full).unwrap();

        let (fp, hello) = fleet.probe(&agent.url, None).await.unwrap();
        assert_eq!(hello.hostname, "fake01");
        let entry = fleet
            .add(&agent.url, None, fp, &code, "Pi garage")
            .await
            .unwrap();
        assert_eq!(entry.id, "pi-garage");
        assert_eq!(agent.hubs.hubs()[0].name, "hub01");
        assert!(
            fleet.probe(&agent.url, None).await.is_err(),
            "already added"
        );

        // A restart reads the same machines and the same key.
        let reloaded = Fleet::load(&config, "hub01").unwrap();
        assert_eq!(reloaded.fingerprint(), fleet.fingerprint());
        let linked = reloaded.get("pi-garage").unwrap();
        assert_eq!(linked.remote.facts().await.unwrap().hostname, "fake01");

        reloaded.rename("pi-garage", "Garage Pi").unwrap();
        assert_eq!(reloaded.get("pi-garage").unwrap().entry.name, "Garage Pi");

        assert!(reloaded.remove("pi-garage").await.unwrap());
        assert!(agent.hubs.hubs().is_empty());
        assert!(Fleet::load(&config, "hub01").unwrap().machines().is_empty());
    }
}
