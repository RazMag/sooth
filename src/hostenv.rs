//! Host environment variables the systemd *user manager* passes on to the
//! quadlet generator -- the set a quadlet file can interpolate as `${NAME}`.
//!
//! sooth manages these through a single `environment.d` drop-in
//! (`~/.config/environment.d/50-sooth.conf`), which is the persistent,
//! user-scoped mechanism systemd reads when the user manager starts. Every
//! mutation is *also* pushed to the already-running manager over D-Bus
//! (`SetEnvironment` / `UnsetEnvironment`, see `systemd::Client`) so it takes
//! effect without a re-login.
//!
//! Variables defined in *other* files under `~/.config/environment.d/` are
//! surfaced read-only for reference. The inherited base environment (`PATH`,
//! `HOME`, the desktop's `XDG_*` soup, ...) is deliberately not listed here
//! -- it isn't something anyone "set", and it's just noise when the point of
//! the page is "what can I reference from a quadlet file".

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Basename of the drop-in sooth owns. The `50-` prefix leaves room for a
/// deliberately higher-numbered user file to still override it, matching
/// systemd's own lexical precedence.
const MANAGED_FILE: &str = "50-sooth.conf";

const HEADER: &str = "\
# Managed by sooth -- edit via the dashboard's Environment page.
# systemd reads this when the user manager starts; sooth also applies
# changes to the running manager immediately.
";

/// One configured variable, as read back from an `environment.d` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVar {
    pub name: String,
    /// The raw value as written in the file (`${...}` left unexpanded).
    pub value: String,
    /// `true` when it lives in sooth's own drop-in (so the UI offers to edit
    /// or remove it); `false` when it comes from another file the user keeps
    /// by hand.
    pub managed: bool,
    /// Basename of the file it was read from.
    pub source: String,
}

/// `~/.config/environment.d` (honouring `XDG_CONFIG_HOME`, same as systemd).
pub fn env_d_dir() -> io::Result<PathBuf> {
    Ok(dirs::config_dir()
        .ok_or_else(|| io::Error::other("could not resolve the user config directory"))?
        .join("environment.d"))
}

/// Full path to the drop-in sooth writes.
pub fn managed_file() -> io::Result<PathBuf> {
    Ok(env_d_dir()?.join(MANAGED_FILE))
}

/// A valid environment variable name: a C identifier (`[A-Za-z_][A-Za-z0-9_]*`).
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A value that survives a round-trip through the single-line `KEY=VALUE`
/// file format.
pub fn valid_value(value: &str) -> bool {
    !value.contains(['\n', '\r', '\0'])
}

/// Every variable configured under `~/.config/environment.d/`, sorted by
/// name. When more than one file defines the same name the later file wins
/// (systemd's rule), and that file is the one reported as the `source`.
pub fn load() -> io::Result<Vec<EnvVar>> {
    load_from(&env_d_dir()?)
}

/// Upsert `name=value` into sooth's drop-in, leaving every other line (and
/// any other file) untouched. Written atomically.
pub fn set(name: &str, value: &str) -> io::Result<()> {
    upsert_in(&env_d_dir()?, name, value)
}

/// Drop every definition of `name` from sooth's drop-in. Returns whether the
/// file actually contained it.
pub fn unset(name: &str) -> io::Result<bool> {
    remove_in(&env_d_dir()?, name)
}

fn load_from(dir: &Path) -> io::Result<Vec<EnvVar>> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "conf"))
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    files.sort();

    let mut by_name: HashMap<String, EnvVar> = HashMap::new();
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let base = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let managed = base == MANAGED_FILE;
        for (name, value) in parse_conf(&text) {
            by_name.insert(
                name.clone(),
                EnvVar {
                    name,
                    value,
                    managed,
                    source: base.clone(),
                },
            );
        }
    }

    let mut out: Vec<EnvVar> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn parse_conf(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            let k = k.trim();
            valid_name(k).then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// Whether `line` is an active (non-comment) assignment to `name`.
fn defines(line: &str, name: &str) -> bool {
    let t = line.trim();
    if t.starts_with('#') || t.starts_with(';') {
        return false;
    }
    t.split_once('=').is_some_and(|(k, _)| k.trim() == name)
}

fn upsert_in(dir: &Path, name: &str, value: &str) -> io::Result<()> {
    let path = dir.join(MANAGED_FILE);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let assignment = format!("{name}={value}");

    let body = if existing.trim().is_empty() {
        format!("{HEADER}{assignment}\n")
    } else {
        let mut lines: Vec<String> = Vec::new();
        let mut replaced = false;
        for line in existing.lines() {
            if defines(line, name) {
                if !replaced {
                    lines.push(assignment.clone());
                    replaced = true;
                }
            } else {
                lines.push(line.to_string());
            }
        }
        if !replaced {
            lines.push(assignment);
        }
        let mut s = lines.join("\n");
        s.push('\n');
        s
    };

    std::fs::create_dir_all(dir)?;
    write_atomic(dir, &path, body.as_bytes())
}

fn remove_in(dir: &Path, name: &str) -> io::Result<bool> {
    let path = dir.join(MANAGED_FILE);
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut lines: Vec<String> = Vec::new();
    let mut removed = false;
    for line in existing.lines() {
        if defines(line, name) {
            removed = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if removed {
        let mut s = lines.join("\n");
        s.push('\n');
        write_atomic(dir, &path, s.as_bytes())?;
    }
    Ok(removed)
}

/// Write to a temp file in the same directory, fsync, then rename into place
/// -- a reader never sees a half-written drop-in.
fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = dir.join(format!(".sooth-env-{}", uuid::Uuid::new_v4()));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(valid_name("app_path"));
        assert!(valid_name("_x"));
        assert!(valid_name("APP1"));
        assert!(!valid_name(""));
        assert!(!valid_name("1abc"));
        assert!(!valid_name("has space"));
        assert!(!valid_name("has-dash"));
        assert!(!valid_name("a=b"));
    }

    #[test]
    fn parse_skips_comments_and_blanks() {
        // `#` / `;` comments and blank lines are ignored; a malformed key
        // ("bad name") is dropped; a `${...}` reference is kept verbatim.
        let text = "# a comment\n\n  ; also a comment\nFOO=bar\n  BAZ=${FOO}/x\nbad name=x\n";
        let got = parse_conf(text);
        assert_eq!(
            got,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "${FOO}/x".to_string()),
            ]
        );
    }

    #[test]
    fn set_creates_file_with_header() {
        let dir = tempfile::tempdir().unwrap();
        upsert_in(dir.path(), "app_path", "/srv/app").unwrap();
        let text = std::fs::read_to_string(dir.path().join(MANAGED_FILE)).unwrap();
        assert!(text.starts_with("# Managed by sooth"));
        assert!(text.contains("\napp_path=/srv/app\n"));
    }

    #[test]
    fn set_replaces_existing_key_in_place_and_keeps_others() {
        let dir = tempfile::tempdir().unwrap();
        upsert_in(dir.path(), "A", "1").unwrap();
        upsert_in(dir.path(), "B", "2").unwrap();
        upsert_in(dir.path(), "A", "99").unwrap();
        let text = std::fs::read_to_string(dir.path().join(MANAGED_FILE)).unwrap();
        assert!(text.contains("\nA=99\n"), "{text}");
        assert!(text.contains("\nB=2\n"), "{text}");
        assert_eq!(text.matches("A=").count(), 1, "{text}");
    }

    #[test]
    fn unset_removes_only_the_named_key() {
        let dir = tempfile::tempdir().unwrap();
        upsert_in(dir.path(), "A", "1").unwrap();
        upsert_in(dir.path(), "B", "2").unwrap();
        assert!(remove_in(dir.path(), "A").unwrap());
        assert!(!remove_in(dir.path(), "A").unwrap());
        let text = std::fs::read_to_string(dir.path().join(MANAGED_FILE)).unwrap();
        assert!(!text.contains("A=1"), "{text}");
        assert!(text.contains("B=2"), "{text}");
    }

    #[test]
    fn load_merges_files_later_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("10-early.conf"), "X=early\nONLY_EARLY=1\n").unwrap();
        std::fs::write(dir.path().join(MANAGED_FILE), "X=sooth\n").unwrap();
        let vars = load_from(dir.path()).unwrap();
        let x = vars.iter().find(|v| v.name == "X").unwrap();
        assert_eq!(x.value, "sooth");
        assert!(x.managed);
        let only = vars.iter().find(|v| v.name == "ONLY_EARLY").unwrap();
        assert!(!only.managed);
        assert_eq!(only.source, "10-early.conf");
    }

    #[test]
    fn load_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_from(&dir.path().join("nope")).unwrap().is_empty());
    }
}
