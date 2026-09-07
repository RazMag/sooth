use std::path::{Path, PathBuf};

use tokio::sync::broadcast;
use tracing::{info, warn};

use super::model::{QuadletUnit, UnitKind};
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

/// Whether `dir` holds a quadlet file whose generated systemd unit is
/// `service`, reversing the naming in `UnitKind::service_infix`
/// (`foo-volume.service` <- `foo.volume`, `bar.service` <- `bar.container`),
/// also matching a template instance against its template file
/// (`foo@bar.service` <- `foo@.container`). A cheap existence check with no
/// parsing, for the status-watch hot path -- it fires for *every* user unit
/// on the bus, most of which sooth doesn't manage.
pub fn has_quadlet_for_service(dir: &Path, service: &str) -> bool {
    let stem = service.strip_suffix(".service").unwrap_or(service);
    UnitKind::all().iter().any(|&kind| {
        // Recover the file stem by peeling off this kind's service infix
        // (empty for container/kube, so those always reach the check).
        let Some(base) = stem.strip_suffix(kind.service_infix()) else {
            return false;
        };
        let ext = kind.extension();
        let mut names = vec![format!("{base}.{ext}")];
        if let Some((prefix, _)) = base.split_once('@') {
            names.push(format!("{prefix}@.{ext}"));
        }
        names.iter().any(|n| dir.join(n).exists())
    })
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

/// A notify event that actually changes the *contents* of the quadlet
/// directory -- a file created, removed, renamed, or written. Crucially this
/// excludes `Access` (open/read/close-without-write) and metadata-only
/// events: sooth reads these files constantly (every list/detail render), and
/// inotify reports each read, so treating reads as changes would have the
/// dashboard trigger a `daemon-reload` + full UI refresh on its own traffic.
fn is_content_change(kind: &notify::EventKind) -> bool {
    use notify::EventKind::{Create, Modify, Remove};
    use notify::event::ModifyKind;
    matches!(
        kind,
        Create(_) | Remove(_) | Modify(ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Any)
    )
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
        let event = match res {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "quadlet directory watch error");
                return;
            }
        };
        if is_content_change(&event.kind) {
            // Send-error just means no one is listening right now; not fatal.
            let _ = changed.send(());
        }
    })?;
    watcher.watch(dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_quadlet_for_service_matches_only_managed_units() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("web.container"), "[Container]\n").unwrap();
        std::fs::write(dir.path().join("data.volume"), "[Volume]\n").unwrap();
        std::fs::write(dir.path().join("tmpl@.container"), "[Container]\n").unwrap();

        assert!(has_quadlet_for_service(dir.path(), "web.service"));
        // a .volume generates <name>-volume.service, not <name>.service
        assert!(has_quadlet_for_service(dir.path(), "data-volume.service"));
        assert!(!has_quadlet_for_service(dir.path(), "data.service"));
        // template instance resolves against the template file
        assert!(has_quadlet_for_service(dir.path(), "tmpl@1.service"));
        // desktop noise -- nothing sooth manages
        assert!(!has_quadlet_for_service(
            dir.path(),
            "plasma-kwin_wayland.service"
        ));
        assert!(!has_quadlet_for_service(dir.path(), "other.service"));
    }

    #[test]
    fn watch_only_reacts_to_content_changes() {
        use notify::EventKind;
        use notify::event::{AccessKind, ModifyKind, RemoveKind};

        assert!(is_content_change(&EventKind::Create(
            notify::event::CreateKind::File
        )));
        assert!(is_content_change(&EventKind::Remove(RemoveKind::File)));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Content
        ))));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Name(
            notify::event::RenameMode::Any
        ))));
        // reads and metadata churn must NOT count -- sooth reads these files
        // on every render
        assert!(!is_content_change(&EventKind::Access(AccessKind::Read)));
        assert!(!is_content_change(&EventKind::Access(AccessKind::Open(
            notify::event::AccessMode::Read
        ))));
        assert!(!is_content_change(&EventKind::Modify(
            ModifyKind::Metadata(notify::event::MetadataKind::AccessTime)
        )));
    }
}
