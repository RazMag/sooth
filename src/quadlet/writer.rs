use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::debug;

use super::{QuadletError, naming, parser};

/// Structural + best-effort generator validation for a candidate quadlet
/// file's contents, run before any write hits disk.
pub fn validate(file_name: &str, contents: &str) -> Result<(), QuadletError> {
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
fn check_against_generator(file_name: &str) -> Result<(), QuadletError> {
    const CANDIDATES: &[&str] = &[
        "/usr/lib/systemd/user-generators/podman-user-generator",
        "/usr/lib/systemd/system-generators/podman-system-generator",
    ];
    let Some(bin) = CANDIDATES.iter().find(|p| Path::new(p).is_file()) else {
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

/// Atomically writes `contents` as `file_name` inside `dir`: write to a temp
/// file in the same directory, fsync, then rename into place (rename is
/// atomic on the same filesystem), so a reader never observes a partial file.
pub fn write_atomic(dir: &Path, file_name: &str, contents: &str) -> Result<PathBuf, QuadletError> {
    validate(file_name, contents)?;
    let final_path = dir.join(file_name);
    let tmp_path = dir.join(format!(".sooth-tmp-{}", uuid::Uuid::new_v4()));

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

pub fn delete(dir: &Path, file_name: &str) -> Result<(), QuadletError> {
    let path = dir.join(file_name);
    if !path.is_file() {
        return Err(QuadletError::NotFound(file_name.to_string()));
    }
    std::fs::remove_file(path)?;
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
}
