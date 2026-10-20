//! Sooth's own port bookkeeping, kept out of the quadlet files: which pod
//! member a pod-published port is assigned to by hand, and which ports a
//! `Network=host` container listens on. Neither means anything to podman or
//! systemd, so it lives in one sidecar file, `.sooth/port-notes.toml` under
//! the quadlet directory, keyed by bare file name (unique tree-wide, see
//! `QuadletUnit::rel_path`):
//!
//! ```toml
//! ["web.container"]
//! serves = ["80/tcp"]
//!
//! ["dns.container"]
//! host = ["53/udp", "8000-8010/tcp"]
//! ```
//!
//! Outside the files because a quadlet in a git-synced group is reset to the
//! remote on every sync, which would throw a local edit away. `.sooth` is a
//! dot-dir, so `discovery` (and its watch) and the podman generator never
//! look in it; an entry survives the unit moving between groups. `discovery`
//! attaches each unit's entry as `QuadletUnit::port_notes` on load.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::QuadletError;
use super::ports::{ExposedPort, parse_exposed};

const DIR: &str = ".sooth";
const FILE: &str = "port-notes.toml";
const HEADER: &str = "# Managed by sooth: pod port assignments and host-network ports, by quadlet\n\
                      # file name. Edit through sooth's Ports screen.\n\n";

/// Serializes read-modify-write cycles on the file, so two concurrent
/// requests can't each write back their own copy and lose the other's.
static LOCK: Mutex<()> = Mutex::new(());

/// One unit's notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PortNotes {
    /// Pod-published container ports assigned to this pod member by hand.
    pub serves: Vec<ExposedPort>,
    /// Ports this `Network=host` container listens on.
    pub host: Vec<ExposedPort>,
}

impl PortNotes {
    pub fn is_empty(&self) -> bool {
        self.serves.is_empty() && self.host.is_empty()
    }
}

/// Every unit's notes, by file name.
pub type Notes = BTreeMap<String, PortNotes>;

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    serves: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    host: Vec<String>,
}

fn path(quadlet_dir: &Path) -> PathBuf {
    quadlet_dir.join(DIR).join(FILE)
}

/// Every unit's notes, for display. Best-effort: a missing file is no
/// notes, and an unreadable one is logged and treated the same, so it can't
/// take a page down.
pub fn load(quadlet_dir: &Path) -> Notes {
    read(quadlet_dir).unwrap_or_else(|e| {
        warn!(file = %path(quadlet_dir).display(), error = %e, "ignoring unreadable port notes");
        Notes::new()
    })
}

/// [`load`], but an unreadable file is an error -- what [`update`] needs,
/// so it never writes back an empty set over notes it just couldn't parse.
fn read(quadlet_dir: &Path) -> Result<Notes, QuadletError> {
    let text = match std::fs::read_to_string(path(quadlet_dir)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Notes::new()),
        Err(e) => return Err(e.into()),
    };
    parse(&text)
}

fn parse(text: &str) -> Result<Notes, QuadletError> {
    let stored: BTreeMap<String, Stored> = toml::from_str(text)
        .map_err(|e| QuadletError::Validation(format!("malformed {DIR}/{FILE}: {e}")))?;
    let ports = |list: &[String]| list.iter().filter_map(|s| parse_exposed(s)).collect();
    Ok(stored
        .into_iter()
        .map(|(file, s)| {
            let notes = PortNotes {
                serves: ports(&s.serves),
                host: ports(&s.host),
            };
            (file, notes)
        })
        .collect())
}

fn render(notes: &Notes) -> Result<String, QuadletError> {
    let specs = |list: &[ExposedPort]| list.iter().map(|p| p.spec()).collect();
    let stored: BTreeMap<&String, Stored> = notes
        .iter()
        .filter(|(_, n)| !n.is_empty())
        .map(|(file, n)| {
            let s = Stored {
                serves: specs(&n.serves),
                host: specs(&n.host),
            };
            (file, s)
        })
        .collect();
    let body = toml::to_string(&stored)
        .map_err(|e| QuadletError::Validation(format!("can't serialize port notes: {e}")))?;
    Ok(format!("{HEADER}{body}"))
}

/// Applies `change` to the stored notes and writes them back atomically
/// (temp file in `.sooth/`, fsync, rename), dropping units left with no
/// notes. Returns whether anything changed; nothing is written if not.
pub fn update(quadlet_dir: &Path, change: impl FnOnce(&mut Notes)) -> Result<bool, QuadletError> {
    let _guard = LOCK.lock().map_err(|_| {
        QuadletError::Io(io::Error::other(
            "port notes lock poisoned by an earlier panic",
        ))
    })?;
    let before = read(quadlet_dir)?;
    let mut notes = before.clone();
    change(&mut notes);
    notes.retain(|_, n| !n.is_empty());
    if notes == before {
        return Ok(false);
    }
    let dir = quadlet_dir.join(DIR);
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join(format!(".sooth-tmp-{}", uuid::Uuid::new_v4()));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(render(&notes)?.as_bytes())?;
    f.sync_all()?;
    drop(f);
    if let Err(e) = std::fs::rename(&tmp, path(quadlet_dir)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(true)
}

/// Adds (`present`) or removes `port` in `list`, keeping it a set.
pub fn set_port(list: &mut Vec<ExposedPort>, port: ExposedPort, present: bool) {
    if present {
        if !list.contains(&port) {
            list.push(port);
        }
    } else {
        list.retain(|p| *p != port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(s: &str) -> ExposedPort {
        parse_exposed(s).unwrap()
    }

    #[test]
    fn missing_file_is_no_notes() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).is_empty());
    }

    #[test]
    fn update_round_trips_and_drops_emptied_units() {
        let dir = tempfile::tempdir().unwrap();
        let changed = update(dir.path(), |n| {
            let web = n.entry("web.container".into()).or_default();
            set_port(&mut web.serves, port("80"), true);
            let dns = n.entry("dns.container".into()).or_default();
            set_port(&mut dns.host, port("53/udp"), true);
            set_port(&mut dns.host, port("8000-8010"), true);
        })
        .unwrap();
        assert!(changed);
        let notes = load(dir.path());
        assert_eq!(notes["web.container"].serves, [port("80/tcp")]);
        assert_eq!(
            notes["dns.container"].host,
            [port("53/udp"), port("8000-8010")]
        );

        // A no-op writes nothing.
        let unchanged = update(dir.path(), |n| {
            let web = n.entry("web.container".into()).or_default();
            set_port(&mut web.serves, port("80/tcp"), true);
        })
        .unwrap();
        assert!(!unchanged);

        update(dir.path(), |n| {
            let web = n.entry("web.container".into()).or_default();
            set_port(&mut web.serves, port("80"), false);
        })
        .unwrap();
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(!text.contains("web.container"), "{text}");
        assert!(text.contains("\"53/udp\""), "{text}");
    }

    #[test]
    fn unparsable_ports_are_skipped_on_read() {
        let notes = parse("[\"a.container\"]\nserves = [\"80\", \"bad\"]\n").unwrap();
        assert_eq!(notes["a.container"].serves, [port("80")]);
    }

    #[test]
    fn a_malformed_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(DIR)).unwrap();
        std::fs::write(path(dir.path()), "not = [toml").unwrap();
        assert!(load(dir.path()).is_empty());
        let result = update(dir.path(), |n| {
            n.entry("a.container".into())
                .or_default()
                .host
                .push(port("80"));
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(path(dir.path())).unwrap(),
            "not = [toml"
        );
    }
}
