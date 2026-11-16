//! The hub's machines: every agent it has paired with, kept in
//! `machines.toml` beside the config file, with this hub's own link key in
//! `link/` next to it. Copying that config folder to another machine moves
//! the hub with all its pairings.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::link::trust::{Permission, now_secs};
use crate::link::{API_VERSION, Fingerprint, Identity};
use crate::machine::api::Hello;
use crate::machine::remote::{self, RemoteMachine};
use crate::quadlet::QuadletError;

/// One paired machine as saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineEntry {
    /// URL-safe, unique among the hub's machines, made from the name.
    pub id: String,
    pub name: String,
    /// `https://host:port` of its agent.
    pub address: String,
    pub fingerprint: Fingerprint,
    /// What the agent granted at pairing.
    pub permission: Permission,
    pub added_at: u64,
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
}

#[derive(Clone)]
pub struct Fleet {
    inner: Arc<Inner>,
}

struct Inner {
    identity: Identity,
    path: PathBuf,
    /// What this hub calls itself to agents (its hostname).
    hub_name: String,
    machines: RwLock<Vec<Linked>>,
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
        let identity = Identity::load_or_create(&crate::link::dir_beside(config_path))?;
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
        let mut machines = Vec::new();
        for entry in entries {
            match RemoteMachine::new(&entry.address, &identity, entry.fingerprint) {
                Ok(remote) => machines.push(Linked { entry, remote }),
                Err(e) => {
                    tracing::warn!(machine = %entry.name, error = %e, "skipping a saved machine")
                }
            }
        }
        Ok(Self {
            inner: Arc::new(Inner {
                identity,
                path,
                hub_name: hub_name.to_string(),
                machines: RwLock::new(machines),
            }),
        })
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
    pub async fn probe(&self, address: &str) -> Result<(Fingerprint, Hello), AppError> {
        let address = address.trim();
        if self.machines().iter().any(|m| m.entry.address == address) {
            return Err(invalid(format!(
                "{address} is already one of your machines"
            )));
        }
        let (fingerprint, hello) = remote::probe(address, &self.inner.identity).await?;
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
        fingerprint: Fingerprint,
        code: &str,
        name: &str,
    ) -> Result<MachineEntry, AppError> {
        let address = address.trim();
        let name = valid_name(name)?;
        let paired = remote::pair(
            address,
            &self.inner.identity,
            fingerprint,
            code,
            &self.inner.hub_name,
        )
        .await?;
        let remote = RemoteMachine::new(address, &self.inner.identity, fingerprint)?;

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
            address: address.to_string(),
            fingerprint,
            permission: paired.permission,
            added_at: now_secs(),
        };
        machines.retain(|m| m.entry.fingerprint != fingerprint);
        machines.push(Linked {
            entry: entry.clone(),
            remote,
        });
        self.save(&machines)?;
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
        tracing::info!(machine = %linked.entry.name, confirmed, "machine unpaired");
        Ok(confirmed)
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
        self.save(&machines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::remote::tests::{fake, spawn_agent};

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

        let (fp, hello) = fleet.probe(&agent.url).await.unwrap();
        assert_eq!(hello.hostname, "fake01");
        let entry = fleet.add(&agent.url, fp, &code, "Pi garage").await.unwrap();
        assert_eq!(entry.id, "pi-garage");
        assert_eq!(agent.hubs.hubs()[0].name, "hub01");
        assert!(fleet.probe(&agent.url).await.is_err(), "already added");

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
