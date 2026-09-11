use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::debug;

use super::{QuadletError, naming, parser};

/// Structural + best-effort generator validation for a candidate quadlet
/// file's contents, run before any write hits disk. `rel_path` may be a bare
/// file name or a `group/.../name` path; only its final component is inspected
/// here, and the generator dry-run keys off that name too.
pub fn validate(rel_path: &str, contents: &str) -> Result<(), QuadletError> {
    let file_name = naming::basename(rel_path);
    if contents.trim().is_empty() {
        return Err(QuadletError::Validation("file is empty".into()));
    }
    let kind = naming::kind_of(file_name).ok_or_else(|| {
        QuadletError::Validation(format!("unrecognized quadlet extension: {file_name}"))
    })?;
    let sections = parser::parse(contents)?;
    let required = kind.primary_section();
    if !sections
        .iter()
        .any(|s| s.name.eq_ignore_ascii_case(required))
    {
        return Err(QuadletError::Validation(format!(
            "missing required [{required}] section for a .{} file",
            kind.extension()
        )));
    }
    check_against_generator(file_name)?;
    Ok(())
}

/// Attempts to validate the file against the real podman quadlet generator
/// (the same code systemd invokes to turn `.container` files into `.service`
/// units), so obviously-wrong content is caught before it reaches systemd.
///
/// This is deliberately conservative: locating and invoking the generator
/// binary correctly varies across podman versions and distros, and its
/// `--dryrun` output describes the whole search directory rather than a
/// single candidate file. When the generator can't be found, or its output
/// can't be confidently attributed to this file (by name), we log and move
/// on rather than rejecting a possibly-valid file on a guess -- the
/// structural check above remains the authoritative gate either way.
/// Fixed install paths for podman's quadlet generator, rootless (user) first
/// since that's sooth's whole scope. Shared with `crate::health`, which
/// surfaces the same absence as a UI warning instead of a silent skip.
const GENERATOR_CANDIDATES: &[&str] = &[
    "/usr/lib/systemd/user-generators/podman-user-generator",
    "/usr/lib/systemd/system-generators/podman-system-generator",
];

/// Whether the real podman quadlet generator is present on this host.
pub fn generator_present() -> bool {
    GENERATOR_CANDIDATES.iter().any(|p| Path::new(p).is_file())
}

fn check_against_generator(file_name: &str) -> Result<(), QuadletError> {
    let Some(bin) = GENERATOR_CANDIDATES.iter().find(|p| Path::new(p).is_file()) else {
        debug!("podman quadlet generator not found on this system; skipping dry-run validation");
        return Ok(());
    };
    match Command::new(bin).arg("--dryrun").output() {
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !out.status.success() && stderr.contains(file_name) {
                return Err(QuadletError::GeneratorRejected(stderr.trim().to_string()));
            }
        }
        Err(e) => debug!(error = %e, "failed to run podman quadlet generator dry-run"),
    }
    Ok(())
}

/// Rejects a `group/.../name` relative path whose directory part isn't a valid
/// group (traversal, reserved `env`, bad characters). A bare file name is
/// always fine here -- the extension is checked in [`validate`].
fn check_rel_path(rel_path: &str) -> Result<(), QuadletError> {
    let group = match rel_path.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => return Ok(()),
    };
    if naming::valid_group(group) {
        Ok(())
    } else {
        Err(QuadletError::Validation(format!(
            "invalid group '{group}': use path segments of letters, digits, '_', '-', '.'; no '..'"
        )))
    }
}

/// Atomically writes `contents` at `rel_path` (a bare file name, or a
/// `group/.../name` path) inside `dir`: create any missing parent
/// directories, write to a temp file *in the target directory*, fsync, then
/// rename into place (rename is atomic on the same filesystem), so a reader
/// never observes a partial file.
pub fn write_atomic(dir: &Path, rel_path: &str, contents: &str) -> Result<PathBuf, QuadletError> {
    check_rel_path(rel_path)?;
    validate(rel_path, contents)?;
    let final_path = dir.join(rel_path);
    let parent = final_path.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent)?;
    let tmp_path = parent.join(format!(".sooth-tmp-{}", uuid::Uuid::new_v4()));

    let mut f = std::fs::File::create(&tmp_path)?;
    f.write_all(contents.as_bytes())?;
    f.sync_all()?;
    drop(f);

    if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.into());
    }
    Ok(final_path)
}

pub fn delete(dir: &Path, rel_path: &str) -> Result<(), QuadletError> {
    let path = dir.join(rel_path);
    if !path.is_file() {
        return Err(QuadletError::NotFound(rel_path.to_string()));
    }
    std::fs::remove_file(&path)?;
    Ok(())
}

/// Moves a quadlet file from `from_rel` to `to_rel` within `dir` (a group
/// change): create the destination directory, then rename (atomic on one
/// filesystem). The group directory the file left is *not* removed even if it
/// is now empty -- groups are user-managed (see `delete_dir`). The caller must
/// have checked that `to_rel` is free.
pub fn move_file(dir: &Path, from_rel: &str, to_rel: &str) -> Result<PathBuf, QuadletError> {
    check_rel_path(to_rel)?;
    let from = dir.join(from_rel);
    if !from.is_file() {
        return Err(QuadletError::NotFound(from_rel.to_string()));
    }
    let to = dir.join(to_rel);
    let parent = to.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent)?;
    std::fs::rename(&from, &to)?;
    Ok(to)
}

/// Re-parents a whole group directory: renames `<dir>/<from_group>` to
/// `<dir>/<to_group>` (every unit inside moves with it, keeping its service
/// name). `to_group` must be a valid group path and must not already exist;
/// the caller is responsible for rejecting a move into the group itself or a
/// descendant. The old parent directory is left in place even if emptied.
pub fn move_dir(dir: &Path, from_group: &str, to_group: &str) -> Result<PathBuf, QuadletError> {
    if !naming::valid_group(to_group) || to_group.is_empty() {
        return Err(QuadletError::Validation(format!(
            "invalid target group '{to_group}'"
        )));
    }
    let from = dir.join(from_group);
    if !from.is_dir() {
        return Err(QuadletError::NotFound(from_group.to_string()));
    }
    let to = dir.join(to_group);
    if to.exists() {
        return Err(QuadletError::Validation(format!(
            "group '{to_group}' already exists"
        )));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&from, &to)?;
    Ok(to)
}

/// Removes a group directory and any empty subdirectory tree under it. The
/// caller must have verified it holds no quadlet files -- this does not check.
pub fn delete_dir(dir: &Path, group: &str) -> Result<(), QuadletError> {
    if group.is_empty() || !naming::valid_group(group) {
        return Err(QuadletError::Validation(format!("invalid group '{group}'")));
    }
    let path = dir.join(group);
    if !path.is_dir() {
        return Err(QuadletError::NotFound(group.to_string()));
    }
    std::fs::remove_dir_all(&path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let contents = "[Container]\nImage=docker.io/library/alpine\n";
        let path = write_atomic(dir.path(), "test.container", contents).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
    }

    #[test]
    fn rejects_missing_primary_section() {
        let dir = tempfile::tempdir().unwrap();
        let err =
            write_atomic(dir.path(), "test.container", "[Unit]\nDescription=x\n").unwrap_err();
        assert!(matches!(err, QuadletError::Validation(_)));
    }

    #[test]
    fn rejects_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(write_atomic(dir.path(), "test.container", "").is_err());
    }

    #[test]
    fn delete_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            delete(dir.path(), "nope.container"),
            Err(QuadletError::NotFound(_))
        ));
    }

    #[test]
    fn write_creates_missing_group_directories() {
        let dir = tempfile::tempdir().unwrap();
        let contents = "[Container]\nImage=alpine\n";
        let path = write_atomic(dir.path(), "media/arr/sonarr.container", contents).unwrap();
        assert_eq!(path, dir.path().join("media/arr/sonarr.container"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        // no stray temp file left in the target directory
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("media/arr"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".sooth-tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn write_rejects_group_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let err =
            write_atomic(dir.path(), "../evil.container", "[Container]\nImage=x\n").unwrap_err();
        assert!(matches!(err, QuadletError::Validation(_)));
        let err =
            write_atomic(dir.path(), "env/x.container", "[Container]\nImage=x\n").unwrap_err();
        assert!(matches!(err, QuadletError::Validation(_)));
    }

    #[test]
    fn move_file_relocates_and_leaves_the_emptied_group_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_atomic(root, "media/arr/sonarr.container", "[Container]\nImage=x\n").unwrap();
        // a sidecar the move must not touch
        std::fs::create_dir_all(root.join("env")).unwrap();
        std::fs::write(root.join("env/sonarr.env"), "A=1\n").unwrap();

        let to = move_file(root, "media/arr/sonarr.container", "infra/sonarr.container").unwrap();
        assert_eq!(to, root.join("infra/sonarr.container"));
        assert!(to.is_file());
        // groups are user-managed: the emptied `media/arr` stays put
        assert!(root.join("media/arr").is_dir());
        assert!(root.join("env/sonarr.env").is_file());
    }

    #[test]
    fn delete_removes_only_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_atomic(root, "solo/only.container", "[Container]\nImage=x\n").unwrap();
        delete(root, "solo/only.container").unwrap();
        assert!(!root.join("solo/only.container").exists());
        assert!(root.join("solo").is_dir()); // the now-empty group is kept
    }

    #[test]
    fn move_dir_reparents_group_with_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_atomic(root, "media/arr/sonarr.container", "[Container]\nImage=x\n").unwrap();
        write_atomic(root, "media/arr/radarr.container", "[Container]\nImage=x\n").unwrap();

        move_dir(root, "media/arr", "infra/arr").unwrap();
        assert!(root.join("infra/arr/sonarr.container").is_file());
        assert!(root.join("infra/arr/radarr.container").is_file());
        assert!(root.join("media").is_dir()); // emptied source parent stays

        // target already present -> rejected
        std::fs::create_dir_all(root.join("taken")).unwrap();
        assert!(matches!(
            move_dir(root, "infra/arr", "taken"),
            Err(QuadletError::Validation(_))
        ));
    }

    #[test]
    fn delete_dir_removes_an_empty_group_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("media/arr/hd")).unwrap();
        delete_dir(root, "media/arr").unwrap();
        assert!(!root.join("media/arr").exists());
        assert!(root.join("media").is_dir());
        assert!(matches!(
            delete_dir(root, "nope"),
            Err(QuadletError::NotFound(_))
        ));
    }
}
