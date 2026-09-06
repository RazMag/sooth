//! The per-container *sidecar env file* -- a `KEY=VALUE` file sooth writes
//! next to a `.container` / `.build` quadlet (`<quadlet_dir>/env/<stem>.env`)
//! and wires into the unit with a single managed `EnvironmentFile=` line.
//!
//! This is distinct from `crate::hostenv`: that manages the systemd *user
//! manager's* environment (the `${NAME}` interpolation source, via
//! `environment.d`); this is one container's own process environment, passed
//! through to `podman run --env-file`.
//!
//! The `env/` subdirectory is deliberately outside what `discovery` scans
//! (it is non-recursive and only recognises the seven quadlet extensions),
//! so these files never show up as units and the podman quadlet generator
//! ignores them too.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

const HEADER: &str = "\
# Managed by sooth -- edit via the quadlet editor's \"Environment variables\".
# Referenced from the unit's primary section as EnvironmentFile=.
";

/// `<quadlet_dir>/env`.
pub fn env_dir(quadlet_dir: &Path) -> PathBuf {
    quadlet_dir.join("env")
}

/// `<quadlet_dir>/env/<stem>.env`.
pub fn path_for(quadlet_dir: &Path, stem: &str) -> PathBuf {
    env_dir(quadlet_dir).join(format!("{stem}.env"))
}

/// The string to place after `EnvironmentFile=` in the unit. `%h/<rel>` when
/// the sidecar resolves under the user's home (the rootless default -- `%h`
/// is a systemd specifier the quadlet generator expands), otherwise the
/// absolute path (a configured non-home `quadlet_dir`).
pub fn reference_value(quadlet_dir: &Path, stem: &str) -> String {
    reference_value_with_home(dirs::home_dir().as_deref(), quadlet_dir, stem)
}

fn reference_value_with_home(home: Option<&Path>, quadlet_dir: &Path, stem: &str) -> String {
    let abs = path_for(quadlet_dir, stem);
    if let Some(home) = home
        && let Ok(rel) = abs.strip_prefix(home)
    {
        return format!("%h/{}", rel.display());
    }
    abs.display().to_string()
}

/// The sidecar's variables as ordered `(name, value)` pairs, or an empty vec
/// when the file does not exist. Blank / `#` / `;` lines are skipped; a line
/// with an invalid name is dropped (mirrors `hostenv::parse_conf`).
pub fn load(quadlet_dir: &Path, stem: &str) -> io::Result<Vec<(String, String)>> {
    match std::fs::read_to_string(path_for(quadlet_dir, stem)) {
        Ok(text) => Ok(parse(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Writes `vars` to the sidecar atomically (temp file in the same directory,
/// fsync, rename). An empty slice removes the file instead -- "no variables"
/// means "no sidecar", so a stale file never lingers.
pub fn save(quadlet_dir: &Path, stem: &str, vars: &[(String, String)]) -> io::Result<()> {
    if vars.is_empty() {
        return delete(quadlet_dir, stem);
    }
    let dir = env_dir(quadlet_dir);
    std::fs::create_dir_all(&dir)?;

    let mut body = String::from(HEADER);
    for (k, v) in vars {
        body.push_str(k);
        body.push('=');
        body.push_str(v);
        body.push('\n');
    }

    let tmp = dir.join(format!(".sooth-env-{}", uuid::Uuid::new_v4()));
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(body.as_bytes())?;
    f.sync_all()?;
    drop(f);
    if let Err(e) = std::fs::rename(&tmp, path_for(quadlet_dir, stem)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Removes the sidecar. A missing file is not an error.
pub fn delete(quadlet_dir: &Path, stem: &str) -> io::Result<()> {
    match std::fs::remove_file(path_for(quadlet_dir, stem)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Adds or removes exactly one managed `EnvironmentFile=<value>` line inside
/// the `[section]` block of a quadlet file's raw text.
///
/// * `present == true`  -- guarantee one such line exists (inserted at the
///   end of the section's key lines, before any trailing blank line);
///   duplicates are collapsed to the first.
/// * `present == false` -- remove every such line from the section.
///
/// Every other line -- other `EnvironmentFile=` entries, comments, blank
/// lines, whitespace -- is left byte-for-byte alone, and text already in the
/// desired state is returned unchanged. Consistent with the rest of sooth,
/// this patches `raw` as text rather than re-serialising the parsed model.
///
/// Round-trips LF text; on a CRLF file the `\r` stays attached to untouched
/// lines and matching still works (comparison is on the trimmed line), but a
/// newly inserted line is bare-LF.
pub fn patch_environment_file(raw: &str, section: &str, value: &str, present: bool) -> String {
    let managed = format!("EnvironmentFile={value}");
    let had_trailing_nl = raw.ends_with('\n');

    let mut lines: Vec<String> = raw.split('\n').map(str::to_string).collect();
    if had_trailing_nl {
        lines.pop(); // the empty element `split` leaves after a final '\n'
    }

    let is_any_header = |l: &str| {
        let t = l.trim();
        t.starts_with('[') && t.ends_with(']')
    };
    let opens_target = |l: &str| {
        l.trim()
            .strip_prefix('[')
            .and_then(|x| x.strip_suffix(']'))
            .is_some_and(|name| name.trim().eq_ignore_ascii_case(section))
    };

    let finish = |lines: Vec<String>| {
        let mut out = lines.join("\n");
        if had_trailing_nl {
            out.push('\n');
        }
        out
    };

    let Some(header_idx) = lines.iter().position(|l| opens_target(l)) else {
        // The primary section is always present in practice (create/edit run
        // `writer::validate` first). This branch is a defensive fallback.
        if present {
            if lines.iter().any(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{section}]"));
            lines.push(managed);
        }
        return finish(lines);
    };

    let body_start = header_idx + 1;
    let body_end = lines[body_start..]
        .iter()
        .position(|l| is_any_header(l))
        .map_or(lines.len(), |p| body_start + p);

    let managed_idx: Vec<usize> = (body_start..body_end)
        .filter(|&i| lines[i].trim() == managed)
        .collect();

    if present {
        if managed_idx.is_empty() {
            let mut ins = body_end;
            while ins > body_start && lines[ins - 1].trim().is_empty() {
                ins -= 1;
            }
            lines.insert(ins, managed);
        } else {
            // keep the first, drop the rest (highest index first)
            for &i in managed_idx[1..].iter().rev() {
                lines.remove(i);
            }
        }
    } else {
        for &i in managed_idx.iter().rev() {
            lines.remove(i);
        }
    }

    finish(lines)
}

/// Parses the editor's `KEY=VALUE` textarea into ordered pairs, the same way
/// [`parse`] reads a file back but *rejecting* malformed input instead of
/// silently dropping it, so the form can show a precise error. Blank / `#` /
/// `;` lines are skipped; order and duplicate keys are preserved. `Err`
/// carries a 1-based line number.
pub fn parse_editor_lines(raw: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            return Err(format!("line {}: expected NAME=VALUE", i + 1));
        };
        let k = k.trim();
        if !valid_name(k) {
            return Err(format!(
                "line {}: '{k}' is not a valid variable name",
                i + 1
            ));
        }
        if !valid_value(v) {
            return Err(format!("line {}: value must be a single line", i + 1));
        }
        out.push((k.to_string(), v.to_string()));
    }
    Ok(out)
}

/// Serialises `(name, value)` pairs back to the editor's textarea form.
pub fn to_editor_lines(vars: &[(String, String)]) -> String {
    vars.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse(text: &str) -> Vec<(String, String)> {
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

/// A valid environment variable name: a C identifier. Restated here (rather
/// than reused from `hostenv`) so `quadlet` stays a lower layer with no
/// dependency on the host-integration module; the semantics are identical.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A value that survives the single-line `KEY=VALUE` file format.
pub fn valid_value(value: &str) -> bool {
    !value.contains(['\n', '\r', '\0'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips_order_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let vars = vec![
            ("FOO".to_string(), "bar".to_string()),
            ("BAZ".to_string(), "${FOO}/x".to_string()),
            ("FOO".to_string(), "again".to_string()),
        ];
        save(dir.path(), "demo", &vars).unwrap();

        let text = std::fs::read_to_string(path_for(dir.path(), "demo")).unwrap();
        assert!(text.starts_with("# Managed by sooth"));
        assert!(text.contains("\nBAZ=${FOO}/x\n"));

        assert_eq!(load(dir.path(), "demo").unwrap(), vars);
        // no stray temp files left behind
        let leftovers: Vec<_> = std::fs::read_dir(env_dir(dir.path()))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".sooth-env-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn save_empty_deletes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "demo", &[("A".into(), "1".into())]).unwrap();
        assert!(path_for(dir.path(), "demo").exists());
        save(dir.path(), "demo", &[]).unwrap();
        assert!(!path_for(dir.path(), "demo").exists());
    }

    #[test]
    fn load_missing_is_empty_and_delete_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path(), "nope").unwrap().is_empty());
        delete(dir.path(), "nope").unwrap();
        delete(dir.path(), "nope").unwrap();
    }

    #[test]
    fn parse_skips_comments_blanks_and_bad_keys() {
        let text = "# c\n\n ; c2\nFOO=bar\n  BAZ=${FOO}\nbad name=x\n1BAD=y\n";
        assert_eq!(
            parse(text),
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "${FOO}".to_string()),
            ]
        );
    }

    #[test]
    fn parse_editor_lines_reports_errors_with_line_numbers() {
        assert_eq!(
            parse_editor_lines("# c\nFOO=bar\n\nBAZ=${FOO}\n").unwrap(),
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "${FOO}".to_string()),
            ]
        );
        assert_eq!(
            parse_editor_lines("FOO=bar\nnope\n").unwrap_err(),
            "line 2: expected NAME=VALUE"
        );
        assert_eq!(
            parse_editor_lines("1BAD=x\n").unwrap_err(),
            "line 1: '1BAD' is not a valid variable name"
        );
        assert_eq!(
            parse_editor_lines("A=x\ry\n").unwrap_err(),
            "line 1: value must be a single line"
        );
    }

    #[test]
    fn reference_value_prefers_home_specifier() {
        let home = Path::new("/home/x");
        let qdir = Path::new("/home/x/.config/containers/systemd");
        assert_eq!(
            reference_value_with_home(Some(home), qdir, "demo"),
            "%h/.config/containers/systemd/env/demo.env"
        );
        // outside home -> absolute
        let outside = Path::new("/etc/containers/systemd");
        assert_eq!(
            reference_value_with_home(Some(home), outside, "demo"),
            "/etc/containers/systemd/env/demo.env"
        );
        // no home resolvable -> absolute
        assert_eq!(
            reference_value_with_home(None, qdir, "demo"),
            "/home/x/.config/containers/systemd/env/demo.env"
        );
    }

    #[test]
    fn patch_inserts_at_end_of_section() {
        let raw = "[Unit]\nDescription=x\n\n[Container]\nImage=alpine\n";
        let out = patch_environment_file(raw, "Container", "%h/env/a.env", true);
        assert_eq!(
            out,
            "[Unit]\nDescription=x\n\n[Container]\nImage=alpine\nEnvironmentFile=%h/env/a.env\n"
        );
    }

    #[test]
    fn patch_inserts_before_trailing_blank_lines_in_section() {
        let raw = "[Container]\nImage=alpine\n\n\n";
        let out = patch_environment_file(raw, "Container", "x.env", true);
        assert_eq!(
            out,
            "[Container]\nImage=alpine\nEnvironmentFile=x.env\n\n\n"
        );
    }

    #[test]
    fn patch_is_idempotent_when_already_present() {
        let raw = "[Container]\nImage=alpine\nEnvironmentFile=x.env\n";
        assert_eq!(patch_environment_file(raw, "Container", "x.env", true), raw);
    }

    #[test]
    fn patch_collapses_duplicates() {
        let raw =
            "[Container]\nImage=alpine\nEnvironmentFile=x.env\nKey=v\nEnvironmentFile=x.env\n";
        let out = patch_environment_file(raw, "Container", "x.env", true);
        assert_eq!(
            out,
            "[Container]\nImage=alpine\nEnvironmentFile=x.env\nKey=v\n"
        );
    }

    #[test]
    fn patch_leaves_hand_written_environmentfile_alone() {
        let raw = "[Container]\nImage=alpine\nEnvironmentFile=/etc/other.env\n";
        let added = patch_environment_file(raw, "Container", "x.env", true);
        assert_eq!(
            added,
            "[Container]\nImage=alpine\nEnvironmentFile=/etc/other.env\nEnvironmentFile=x.env\n"
        );
        let removed = patch_environment_file(&added, "Container", "x.env", false);
        assert_eq!(removed, raw);
    }

    #[test]
    fn patch_removal_keeps_surrounding_structure() {
        let raw = "[Container]\nImage=alpine\nEnvironmentFile=x.env\n\n[Install]\nWantedBy=default.target\n";
        let out = patch_environment_file(raw, "Container", "x.env", false);
        assert_eq!(
            out,
            "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\n"
        );
    }

    #[test]
    fn patch_matches_header_case_insensitively_and_with_trailing_space() {
        let raw = "[container] \nImage=alpine\n";
        let out = patch_environment_file(raw, "Container", "x.env", true);
        assert_eq!(out, "[container] \nImage=alpine\nEnvironmentFile=x.env\n");
    }

    #[test]
    fn patch_removal_with_nothing_to_remove_is_unchanged() {
        let raw = "[Container]\nImage=alpine\n";
        assert_eq!(
            patch_environment_file(raw, "Container", "x.env", false),
            raw
        );
    }
}
