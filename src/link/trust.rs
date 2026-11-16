//! The agent's side of pairing: the hubs it has pinned (`link/hubs.toml`)
//! and the one pairing code it will currently accept (`link/pair-code.toml`).
//!
//! A code is made on the agent -- by `sooth --pair-code`, or at startup when
//! no hub is paired yet -- and typed into the hub's Add machine form. It
//! expires after [`CODE_LIFETIME_SECS`], works once, and carries the
//! permission the hub will get, so read-only access is the agent owner's
//! choice, not the hub's.

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{Fingerprint, write_private};

/// What a paired hub may do on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    Full,
    ReadOnly,
}

impl Permission {
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::ReadOnly => "Read-only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedHub {
    pub fingerprint: Fingerprint,
    /// What the hub called itself when it paired (its hostname).
    pub name: String,
    pub permission: Permission,
    pub paired_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct HubsFile {
    #[serde(default, rename = "hub")]
    hubs: Vec<TrustedHub>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodeFile {
    code: String,
    expires_at: u64,
    permission: Permission,
}

pub const CODE_LIFETIME_SECS: u64 = 600;

/// Wrong codes tolerated before the current one is thrown away.
const MAX_FAILED_ATTEMPTS: u32 = 10;

/// No 0/O or 1/I, so a code read off a terminal types back unambiguously.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Uppercase, without the dashes and spaces a person might type.
fn normalize(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Why a pairing attempt was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PairRefused {
    NoCode,
    Expired,
    Wrong,
}

impl PairRefused {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NoCode => {
                "this machine has no pairing code -- run `sooth --pair-code` on it to make one"
            }
            Self::Expired => "that pairing code has expired -- run `sooth --pair-code` again",
            Self::Wrong => "that pairing code is wrong",
        }
    }
}

pub struct HubStore {
    dir: PathBuf,
    hubs: RwLock<Vec<TrustedHub>>,
    failed: AtomicU32,
}

impl HubStore {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("hubs.toml");
        let hubs = match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str::<HubsFile>(&text)
                    .map_err(|e| anyhow::anyhow!("{} is not valid: {e}", path.display()))?
                    .hubs
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            hubs: RwLock::new(hubs),
            failed: AtomicU32::new(0),
        })
    }

    pub fn hubs(&self) -> Vec<TrustedHub> {
        self.hubs.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn find(&self, fingerprint: &Fingerprint) -> Option<TrustedHub> {
        self.hubs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|h| &h.fingerprint == fingerprint)
            .cloned()
    }

    fn save(&self, hubs: &[TrustedHub]) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(&HubsFile {
            hubs: hubs.to_vec(),
        })?;
        write_private(&self.dir.join("hubs.toml"), &text)
    }

    /// Makes a new pairing code, replacing any earlier one, and returns it
    /// formatted for reading out (`ABCD-EFGH-JKLM`).
    pub fn issue_code(&self, permission: Permission) -> anyhow::Result<String> {
        issue_code(&self.dir, permission)
    }

    /// Pins `fingerprint` as a hub if `code` is the current, unexpired code.
    /// The code is used up either way it succeeds, and after too many wrong
    /// guesses.
    pub fn pair(
        &self,
        code: &str,
        fingerprint: Fingerprint,
        name: &str,
    ) -> Result<TrustedHub, PairRefused> {
        let path = self.dir.join("pair-code.toml");
        let current: CodeFile = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .ok_or(PairRefused::NoCode)?;
        if current.expires_at <= now_secs() {
            let _ = std::fs::remove_file(&path);
            return Err(PairRefused::Expired);
        }
        let given = normalize(code);
        if !crate::machine::api::same_secret(&given, &normalize(&current.code)) {
            if self.failed.fetch_add(1, Ordering::SeqCst) + 1 >= MAX_FAILED_ATTEMPTS {
                tracing::warn!("too many wrong pairing codes; the current code is cancelled");
                let _ = std::fs::remove_file(&path);
                self.failed.store(0, Ordering::SeqCst);
            }
            return Err(PairRefused::Wrong);
        }
        let _ = std::fs::remove_file(&path);
        self.failed.store(0, Ordering::SeqCst);

        let hub = TrustedHub {
            fingerprint,
            name: name.chars().take(64).collect(),
            permission: current.permission,
            paired_at: now_secs(),
        };
        let mut hubs = self.hubs.write().unwrap_or_else(|e| e.into_inner());
        hubs.retain(|h| h.fingerprint != fingerprint);
        hubs.push(hub.clone());
        if let Err(e) = self.save(&hubs) {
            tracing::error!(error = %e, "couldn't save the paired hub");
        }
        Ok(hub)
    }

    /// Forgets a hub (it unpaired itself).
    pub fn remove(&self, fingerprint: &Fingerprint) -> anyhow::Result<()> {
        let mut hubs = self.hubs.write().unwrap_or_else(|e| e.into_inner());
        hubs.retain(|h| &h.fingerprint != fingerprint);
        self.save(&hubs)
    }
}

/// [`HubStore::issue_code`] without a loaded store, for `sooth --pair-code`
/// (a separate process from the running agent, which reads the file when a
/// hub pairs).
pub fn issue_code(dir: &Path, permission: Permission) -> anyhow::Result<String> {
    let raw: String = rand::random::<[u8; 12]>()
        .iter()
        .map(|b| CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()] as char)
        .collect();
    let code = format!("{}-{}-{}", &raw[0..4], &raw[4..8], &raw[8..12]);
    let file = CodeFile {
        code: code.clone(),
        expires_at: now_secs() + CODE_LIFETIME_SECS,
        permission,
    };
    write_private(&dir.join("pair-code.toml"), &toml::to_string(&file)?)?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::Identity;

    fn fp() -> Fingerprint {
        Identity::generate().unwrap().fingerprint
    }

    #[test]
    fn a_code_pairs_once_and_carries_its_permission() {
        let dir = tempfile::tempdir().unwrap();
        let store = HubStore::load(dir.path()).unwrap();
        let hub = fp();
        assert_eq!(store.pair("x", hub, "hub"), Err(PairRefused::NoCode));

        let code = store.issue_code(Permission::ReadOnly).unwrap();
        assert_eq!(code.len(), 14);
        assert_eq!(store.pair("WRONG", hub, "hub"), Err(PairRefused::Wrong));
        let paired = store
            .pair(&code.to_lowercase().replace('-', " "), hub, "hub01")
            .unwrap();
        assert_eq!(paired.permission, Permission::ReadOnly);
        assert_eq!(store.find(&hub).unwrap().name, "hub01");
        // Used up.
        assert_eq!(store.pair(&code, fp(), "other"), Err(PairRefused::NoCode));

        // Pins survive a reload; removing one forgets it.
        let reloaded = HubStore::load(dir.path()).unwrap();
        assert_eq!(reloaded.hubs(), vec![paired]);
        reloaded.remove(&hub).unwrap();
        assert!(HubStore::load(dir.path()).unwrap().hubs().is_empty());
    }

    #[test]
    fn too_many_wrong_codes_cancel_the_code() {
        let dir = tempfile::tempdir().unwrap();
        let store = HubStore::load(dir.path()).unwrap();
        let code = store.issue_code(Permission::Full).unwrap();
        for _ in 0..MAX_FAILED_ATTEMPTS {
            assert_eq!(store.pair("NOPE", fp(), "x"), Err(PairRefused::Wrong));
        }
        assert_eq!(store.pair(&code, fp(), "x"), Err(PairRefused::NoCode));
    }

    #[test]
    fn an_expired_code_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = HubStore::load(dir.path()).unwrap();
        let file = CodeFile {
            code: "AAAA-BBBB-CCCC".into(),
            expires_at: now_secs() - 1,
            permission: Permission::Full,
        };
        write_private(
            &dir.path().join("pair-code.toml"),
            &toml::to_string(&file).unwrap(),
        )
        .unwrap();
        assert_eq!(
            store.pair("AAAA-BBBB-CCCC", fp(), "x"),
            Err(PairRefused::Expired)
        );
    }
}
