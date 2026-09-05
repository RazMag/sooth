use std::path::{Path, PathBuf};

use tokio::sync::broadcast;
use tracing::{info, warn};

use super::model::QuadletUnit;
use super::{QuadletError, naming, parser};

const EXTENSIONS: &[&str] = &[
    "container",
    "volume",
    "network",
    "pod",
    "kube",
    "build",
    "image",
];

/// Resolves the rootless quadlet directory: `$XDG_CONFIG_HOME/containers/systemd`,
/// falling back to `~/.config/containers/systemd` when unset. This is the one
/// user-writable quadlet search path this app manages -- not the admin-installed
/// `/etc` or `/usr` locations, and not `.d/` drop-ins (out of scope for v1).
pub fn default_quadlet_dir() -> Result<PathBuf, QuadletError> {
    let config_dir = dirs::config_dir().ok_or_else(|| {
        QuadletError::Validation("could not determine the user config directory (no $HOME)".into())
    })?;
    Ok(config_dir.join("containers").join("systemd"))
}

/// Ensures the directory exists, creating it if necessary.
pub fn ensure_dir(dir: &Path) -> Result<(), QuadletError> {
    if !dir.exists() {
        info!(path = %dir.display(), "quadlet directory does not exist, creating it");
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// Enumerates and parses every quadlet unit file directly inside `dir`
/// (non-recursive; `.d/` drop-in directories are not descended into). A file
/// that fails to parse is skipped with a warning rather than failing the
/// whole listing, so one bad file doesn't take down the dashboard.
pub fn load_all(dir: &Path) -> Result<Vec<QuadletUnit>, QuadletError> {
    let mut units = Vec::new();
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(units),
        Err(e) => return Err(e.into()),
    };
    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(ext) = naming::extension(file_name) else {
            continue;
        };
        if !EXTENSIONS.contains(&ext) {
            continue;
        }
        match load_one(&path, file_name) {
            Ok(unit) => units.push(unit),
            Err(e) => warn!(file = file_name, error = %e, "skipping unparsable quadlet file"),
        }
    }
    units.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(units)
}

fn load_one(path: &Path, file_name: &str) -> Result<QuadletUnit, QuadletError> {
    let raw = std::fs::read_to_string(path)?;
    let kind = naming::kind_of(file_name).ok_or_else(|| {
        QuadletError::Validation(format!("unrecognized quadlet extension: {file_name}"))
    })?;
    let sections = parser::parse(&raw)?;
    Ok(QuadletUnit {
        file_name: file_name.to_string(),
        path: path.to_path_buf(),
        kind,
        sections,
        raw,
    })
}

pub fn load_by_name(dir: &Path, file_name: &str) -> Result<QuadletUnit, QuadletError> {
    let path = dir.join(file_name);
    if !path.is_file() {
        return Err(QuadletError::NotFound(file_name.to_string()));
    }
    load_one(&path, file_name)
}

/// Watches the quadlet directory for external changes (edits made outside
/// the dashboard) and notifies `changed` so callers can debounce, re-enumerate,
/// and trigger a systemd reload. Runs for as long as the returned watcher is
/// kept alive.
pub fn watch(
    dir: &Path,
    changed: broadcast::Sender<()>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Err(e) = res {
            warn!(error = %e, "quadlet directory watch error");
            return;
        }
        // Send-error just means no one is listening right now; not fatal.
        let _ = changed.send(());
    })?;
    watcher.watch(dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}
