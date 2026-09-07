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

/// A directory name `load_all` never descends into: the sidecar env-file
/// directory, systemd `.d/` drop-in directories (out of scope), and anything
/// hidden (covers `.sooth-tmp-*` and dot-directories).
fn is_skipped_dir(name: &str) -> bool {
    name == "env" || name.ends_with(".d") || name.starts_with('.')
}

/// Recursively collects the paths of every quadlet file under `dir`, skipping
/// the directories `is_skipped_dir` names and hidden files. A directory that
/// can't be read is skipped with a warning rather than failing the walk.
fn walk_quadlet_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), QuadletError> {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if !is_skipped_dir(name) {
                walk_quadlet_files(&path, out)?;
            }
            continue;
        }
        if !file_type.is_file() || name.starts_with('.') {
            continue;
        }
        if naming::extension(name).is_some_and(|ext| EXTENSIONS.contains(&ext)) {
            out.push(path);
        }
    }
    Ok(())
}

/// The `/`-separated subdirectory `path` sits in, relative to `root` (`""`
/// when `path` is directly inside `root`).
fn group_of(root: &Path, path: &Path) -> String {
    path.parent()
        .and_then(|parent| parent.strip_prefix(root).ok())
        .map(|rel| {
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default()
}

/// Whether `dir` (searched recursively) holds a quadlet file whose generated
/// systemd unit is `service`, reversing the naming in
/// `UnitKind::service_infix` (`foo-volume.service` <- `foo.volume`,
/// `bar.service` <- `bar.container`), also matching a template instance
/// against its template file (`foo@bar.service` <- `foo@.container`). A cheap
/// existence check with no parsing, for the status-watch hot path -- it fires
/// for *every* user unit on the bus, most of which sooth doesn't manage. The
/// quadlet tree is small (a personal dashboard), so the recursive `read_dir`
/// per event is negligible.
pub fn has_quadlet_for_service(dir: &Path, service: &str) -> bool {
    let stem = service.strip_suffix(".service").unwrap_or(service);
    let mut paths = Vec::new();
    if walk_quadlet_files(dir, &mut paths).is_err() {
        return false;
    }
    let names: std::collections::HashSet<&str> = paths
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .collect();
    UnitKind::all().iter().any(|&kind| {
        // Recover the file stem by peeling off this kind's service infix
        // (empty for container/kube, so those always reach the check).
        let Some(base) = stem.strip_suffix(kind.service_infix()) else {
            return false;
        };
        let ext = kind.extension();
        let mut candidates = vec![format!("{base}.{ext}")];
        if let Some((prefix, _)) = base.split_once('@') {
            candidates.push(format!("{prefix}@.{ext}"));
        }
        candidates.iter().any(|c| names.contains(c.as_str()))
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

/// Enumerates and parses every quadlet unit file under `dir`, descending into
/// subdirectories (the "groups" a unit can be filed under) but not into the
/// `env/` sidecar directory or `.d/` drop-in directories. A file that fails to
/// parse is skipped with a warning rather than failing the whole listing, so
/// one bad file doesn't take down the dashboard. Sorted by `(group, file_name)`.
pub fn load_all(dir: &Path) -> Result<Vec<QuadletUnit>, QuadletError> {
    let mut paths = Vec::new();
    walk_quadlet_files(dir, &mut paths)?;
    let mut units = Vec::new();
    for path in paths {
        match load_one(dir, &path) {
            Ok(unit) => units.push(unit),
            Err(e) => {
                warn!(file = %path.display(), error = %e, "skipping unparsable quadlet file")
            }
        }
    }
    units.sort_by(|a, b| {
        (a.group.as_str(), a.file_name.as_str()).cmp(&(b.group.as_str(), b.file_name.as_str()))
    });
    Ok(units)
}

fn load_one(root: &Path, path: &Path) -> Result<QuadletUnit, QuadletError> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            QuadletError::Validation(format!("non-UTF-8 quadlet file name: {}", path.display()))
        })?
        .to_string();
    let raw = std::fs::read_to_string(path)?;
    let kind = naming::kind_of(&file_name).ok_or_else(|| {
        QuadletError::Validation(format!("unrecognized quadlet extension: {file_name}"))
    })?;
    let sections = parser::parse(&raw)?;
    Ok(QuadletUnit {
        group: group_of(root, path),
        file_name,
        path: path.to_path_buf(),
        kind,
        sections,
        raw,
    })
}

/// The on-disk path of the quadlet file named `file_name` (a bare basename),
/// searched across the whole group tree. Podman requires the basename to be
/// unique tree-wide (two `web.container` files would both generate
/// `web.service`), so this is unambiguous in practice; a stray duplicate
/// resolves to the first match with a warning.
pub fn find_in_tree(dir: &Path, file_name: &str) -> Option<PathBuf> {
    let mut paths = Vec::new();
    walk_quadlet_files(dir, &mut paths).ok()?;
    let mut matches = paths
        .into_iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(file_name));
    let first = matches.next()?;
    if matches.next().is_some() {
        warn!(
            file = file_name,
            "multiple quadlet files share this name across groups; using the first"
        );
    }
    Some(first)
}

pub fn load_by_name(dir: &Path, file_name: &str) -> Result<QuadletUnit, QuadletError> {
    let path = find_in_tree(dir, file_name)
        .ok_or_else(|| QuadletError::NotFound(file_name.to_string()))?;
    load_one(dir, &path)
}

/// Whether the group directory `<dir>/<group>` contains at least one quadlet
/// file anywhere below it -- the gate on deleting a group.
pub fn group_has_units(dir: &Path, group: &str) -> bool {
    let mut paths = Vec::new();
    let _ = walk_quadlet_files(&dir.join(group), &mut paths);
    !paths.is_empty()
}

/// Every group directory under `dir`, sorted -- *including empty ones* (a
/// group the user created but hasn't filed anything into yet) and every
/// ancestor path. This is the set the UI offers in its "move to group" /
/// "Add group" controls and renders as (possibly empty) collapsible sections.
pub fn list_groups(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect_group_dirs(dir, dir, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_group_dirs(root: &Path, cur: &Path, out: &mut Vec<String>) {
    let Ok(read_dir) = std::fs::read_dir(cur) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_skipped_dir(name) || !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root) {
            let g = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if !g.is_empty() {
                out.push(g);
            }
        }
        collect_group_dirs(root, &path, out);
    }
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

/// Whether a notify event path is one `load_all` would actually surface -- not
/// inside `env/`, a `.d/` drop-in, or a hidden directory, and not a hidden
/// file (our own `.sooth-tmp-*`). Paths outside the watched tree are treated
/// as relevant (conservative).
fn is_watched_path(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return true;
    };
    !rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == "env" || s.ends_with(".d") || s.starts_with('.')
    })
}

/// Watches the quadlet directory tree for external changes (edits made outside
/// the dashboard) and notifies `changed` so callers can debounce, re-enumerate,
/// and trigger a systemd reload. Recursive, so changes inside group
/// subdirectories are seen; events under `env/` / `.d/` / dot-paths are
/// ignored so the sidecar and drop-ins don't drive reload churn. Runs for as
/// long as the returned watcher is kept alive.
pub fn watch(
    dir: &Path,
    changed: broadcast::Sender<()>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};

    let root = dir.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let event = match res {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "quadlet directory watch error");
                return;
            }
        };
        if is_content_change(&event.kind) && event.paths.iter().any(|p| is_watched_path(&root, p)) {
            // Send-error just means no one is listening right now; not fatal.
            let _ = changed.send(());
        }
    })?;
    watcher.watch(dir, RecursiveMode::Recursive)?;
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
    fn load_all_descends_into_groups_and_skips_env_and_dropins() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("top.container"), "[Container]\nImage=x\n").unwrap();
        std::fs::create_dir_all(root.join("media/arr")).unwrap();
        std::fs::write(root.join("media/plex.container"), "[Container]\nImage=x\n").unwrap();
        std::fs::write(
            root.join("media/arr/sonarr.container"),
            "[Container]\nImage=x\n",
        )
        .unwrap();
        // ignored: the sidecar dir, a drop-in dir, a hidden file, a non-quadlet file
        std::fs::create_dir_all(root.join("env")).unwrap();
        std::fs::write(root.join("env/plex.env"), "A=1\n").unwrap();
        std::fs::create_dir_all(root.join("web.container.d")).unwrap();
        std::fs::write(root.join("web.container.d/override.conf"), "[Container]\n").unwrap();
        std::fs::write(root.join(".sooth-tmp-abc"), "junk").unwrap();
        std::fs::write(root.join("notes.txt"), "hi").unwrap();

        let units = load_all(root).unwrap();
        let by_name: Vec<(&str, &str)> = units
            .iter()
            .map(|u| (u.group.as_str(), u.file_name.as_str()))
            .collect();
        assert_eq!(
            by_name,
            vec![
                ("", "top.container"),
                ("media", "plex.container"),
                ("media/arr", "sonarr.container"),
            ]
        );

        let sonarr = load_by_name(root, "sonarr.container").unwrap();
        assert_eq!(sonarr.group, "media/arr");
        assert_eq!(sonarr.rel_path(), "media/arr/sonarr.container");
        assert!(matches!(
            load_by_name(root, "missing.container"),
            Err(QuadletError::NotFound(_))
        ));

        // status-watch existence check reaches into subdirectories
        assert!(has_quadlet_for_service(root, "sonarr.service"));
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
