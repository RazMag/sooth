//! The `[Container]` `AutoUpdate=` key -- podman's per-container auto-update
//! policy.
//!
//! `AutoUpdate=registry` / `AutoUpdate=local` in a `.container` quadlet is the
//! native form of the `io.containers.autoupdate` label: the quadlet generator
//! turns it into `--label io.containers.autoupdate=<policy>` on the generated
//! `.service`, and `podman auto-update` (run by `podman-auto-update.timer`)
//! acts on containers carrying that label. `registry` pulls a fresh image when
//! the registry has a newer digest; `local` only swaps in an image that's
//! already been pulled.
//!
//! Consistent with the rest of sooth (see [`super::install`],
//! [`super::envfile`]), [`set_autoupdate`] patches the raw file text rather
//! than re-serialising the parsed model, so comments and formatting the app
//! didn't touch survive byte-for-byte.
//!
//! The *current* policy is read straight from the parsed model
//! (`unit.section("Container").and_then(|s| s.get("AutoUpdate"))`) -- unlike
//! autostart, there's no systemd round-trip involved.

// TODO: `install::set_enabled` and `envfile::patch_environment_file` do the
// same "find a `[Section]`, splice one managed line" line-vector scan. This is
// the third copy; if a fourth shows up, extract a shared helper.

const SECTION: &str = "Container";
const KEY: &str = "AutoUpdate";

/// A podman auto-update policy. `None` (no `AutoUpdate=` line) is "off".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoUpdateMode {
    /// Check the registry for a newer image digest and pull it.
    Registry,
    /// Only update to an image that's already present locally.
    Local,
}

impl AutoUpdateMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Local => "local",
        }
    }

    /// Parses a `AutoUpdate=` value (case-insensitive). Returns `None` for the
    /// empty string, `disabled`, or anything podman doesn't recognise -- all of
    /// which the UI treats as "off".
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "registry" => Some(Self::Registry),
            "local" => Some(Self::Local),
            _ => None,
        }
    }
}

fn is_any_header(l: &str) -> bool {
    let t = l.trim();
    t.starts_with('[') && t.ends_with(']')
}

fn opens_container(l: &str) -> bool {
    l.trim()
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .is_some_and(|name| name.trim().eq_ignore_ascii_case(SECTION))
}

/// True when `line` is an `AutoUpdate=` assignment (any value, case-insensitive
/// key, leading/trailing whitespace tolerated).
fn is_autoupdate_line(line: &str) -> bool {
    line.trim()
        .split_once('=')
        .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(KEY))
}

/// Set (`Some`) or clear (`None`) the single managed `AutoUpdate=<mode>` line in
/// `raw`'s `[Container]` section, leaving every other line -- comments, blank
/// lines, other keys -- byte-for-byte alone. Text already in the desired state
/// is returned unchanged.
///
/// * `Some(mode)`: if the section already has an `AutoUpdate=` line, the first
///   is rewritten to `AutoUpdate=<mode>` and any duplicates are dropped;
///   otherwise `AutoUpdate=<mode>` is appended to the section's key lines
///   (before any trailing blank line). The `[Container]` section is required,
///   so in practice it always exists; a fresh one at EOF is a defensive
///   fallback.
/// * `None`: every `AutoUpdate=` line is removed from the section. The section
///   header itself is never removed (it's the file's required primary section).
///
/// Round-trips LF text; on a CRLF file the `\r` stays attached to untouched
/// lines (matching is on the trimmed line) but a newly inserted line is
/// bare-LF, mirroring [`super::install::set_enabled`].
pub fn set_autoupdate(raw: &str, mode: Option<AutoUpdateMode>) -> String {
    let had_trailing_nl = raw.ends_with('\n');
    let mut lines: Vec<String> = raw.split('\n').map(str::to_string).collect();
    if had_trailing_nl {
        lines.pop(); // the empty element `split` leaves after a final '\n'
    }

    let finish = |lines: Vec<String>| {
        let mut out = lines.join("\n");
        if had_trailing_nl {
            out.push('\n');
        }
        out
    };

    let header_idx = lines.iter().position(|l| opens_container(l));

    let Some(h) = header_idx else {
        // No `[Container]` section. A real container file always has one
        // (create/edit run `writer::validate` first); this is a defensive
        // fallback so a hand-mangled file still behaves.
        if let Some(mode) = mode {
            if lines.iter().any(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{SECTION}]"));
            lines.push(format!("{KEY}={}", mode.as_str()));
        }
        return finish(lines);
    };

    let body_start = h + 1;
    let body_end = lines[body_start..]
        .iter()
        .position(|l| is_any_header(l))
        .map_or(lines.len(), |p| body_start + p);

    let hits: Vec<usize> = (body_start..body_end)
        .filter(|&i| is_autoupdate_line(&lines[i]))
        .collect();

    match mode {
        Some(mode) => {
            let desired = format!("{KEY}={}", mode.as_str());
            match hits.split_first() {
                Some((&first, rest)) => {
                    lines[first] = desired;
                    for &i in rest.iter().rev() {
                        lines.remove(i);
                    }
                }
                None => {
                    let mut ins = body_end;
                    while ins > body_start && lines[ins - 1].trim().is_empty() {
                        ins -= 1;
                    }
                    lines.insert(ins, desired);
                }
            }
        }
        None => {
            for &i in hits.iter().rev() {
                lines.remove(i);
            }
        }
    }

    finish(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_case_insensitive_and_defaults_off() {
        assert_eq!(
            AutoUpdateMode::parse("registry"),
            Some(AutoUpdateMode::Registry)
        );
        assert_eq!(
            AutoUpdateMode::parse("  LOCAL "),
            Some(AutoUpdateMode::Local)
        );
        assert_eq!(AutoUpdateMode::parse(""), None);
        assert_eq!(AutoUpdateMode::parse("off"), None);
        assert_eq!(AutoUpdateMode::parse("registry.example.com"), None);
    }

    #[test]
    fn set_appends_to_a_bare_container_section() {
        let raw = "[Container]\nImage=alpine\n";
        assert_eq!(
            set_autoupdate(raw, Some(AutoUpdateMode::Registry)),
            "[Container]\nImage=alpine\nAutoUpdate=registry\n"
        );
    }

    #[test]
    fn set_is_idempotent_when_already_in_that_state() {
        let raw = "[Container]\nImage=alpine\nAutoUpdate=registry\n";
        assert_eq!(set_autoupdate(raw, Some(AutoUpdateMode::Registry)), raw);
    }

    #[test]
    fn set_rewrites_an_existing_value_in_place() {
        let raw = "[Container]\nImage=alpine\nAutoUpdate=local\nLabel=x=y\n";
        assert_eq!(
            set_autoupdate(raw, Some(AutoUpdateMode::Registry)),
            "[Container]\nImage=alpine\nAutoUpdate=registry\nLabel=x=y\n"
        );
    }

    #[test]
    fn set_collapses_duplicates_to_the_first() {
        let raw = "[Container]\nAutoUpdate=local\nImage=alpine\nAutoUpdate=registry\n";
        assert_eq!(
            set_autoupdate(raw, Some(AutoUpdateMode::Local)),
            "[Container]\nAutoUpdate=local\nImage=alpine\n"
        );
    }

    #[test]
    fn clear_removes_the_line_and_leaves_the_rest() {
        let raw = "[Container]\nImage=alpine\nAutoUpdate=registry\nLabel=x=y\n";
        assert_eq!(
            set_autoupdate(raw, None),
            "[Container]\nImage=alpine\nLabel=x=y\n"
        );
    }

    #[test]
    fn clear_is_a_noop_when_absent() {
        let raw = "[Container]\nImage=alpine\n";
        assert_eq!(set_autoupdate(raw, None), raw);
    }

    #[test]
    fn clear_removes_every_duplicate() {
        let raw = "[Container]\nAutoUpdate=local\nImage=alpine\nAutoUpdate=registry\n";
        assert_eq!(set_autoupdate(raw, None), "[Container]\nImage=alpine\n");
    }

    #[test]
    fn only_the_container_section_is_touched() {
        // an `AutoUpdate=` in some other section is left strictly alone
        let raw = "[Service]\nAutoUpdate=nonsense\n\n[Container]\nImage=alpine\n";
        assert_eq!(
            set_autoupdate(raw, Some(AutoUpdateMode::Registry)),
            "[Service]\nAutoUpdate=nonsense\n\n[Container]\nImage=alpine\nAutoUpdate=registry\n"
        );
    }

    #[test]
    fn insert_goes_before_a_trailing_blank_line_in_the_section() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\n";
        assert_eq!(
            set_autoupdate(raw, Some(AutoUpdateMode::Registry)),
            "[Container]\nImage=alpine\nAutoUpdate=registry\n\n[Install]\nWantedBy=default.target\n"
        );
    }

    #[test]
    fn header_match_tolerates_case_and_trailing_space() {
        let raw = "[container] \nImage=alpine\n";
        assert_eq!(
            set_autoupdate(raw, Some(AutoUpdateMode::Local)),
            "[container] \nImage=alpine\nAutoUpdate=local\n"
        );
    }

    #[test]
    fn key_match_tolerates_case_and_spacing() {
        let raw = "[Container]\nImage=alpine\n  autoupdate = local\n";
        assert_eq!(
            set_autoupdate(raw, Some(AutoUpdateMode::Registry)),
            "[Container]\nImage=alpine\nAutoUpdate=registry\n"
        );
    }

    #[test]
    fn preserves_a_file_with_no_trailing_newline() {
        let raw = "[Container]\nImage=alpine";
        assert_eq!(
            set_autoupdate(raw, Some(AutoUpdateMode::Registry)),
            "[Container]\nImage=alpine\nAutoUpdate=registry"
        );
    }

    #[test]
    fn comments_survive_untouched() {
        let raw = "[Container]\n# keep me\nImage=alpine\n";
        assert_eq!(
            set_autoupdate(raw, Some(AutoUpdateMode::Registry)),
            "[Container]\n# keep me\nImage=alpine\nAutoUpdate=registry\n"
        );
        assert_eq!(
            set_autoupdate(
                "[Container]\n# keep me\nImage=alpine\nAutoUpdate=local\n",
                None
            ),
            "[Container]\n# keep me\nImage=alpine\n"
        );
    }

    #[test]
    fn defensive_fallback_when_no_container_section() {
        assert_eq!(
            set_autoupdate("[Volume]\nName=data\n", Some(AutoUpdateMode::Registry)),
            "[Volume]\nName=data\n\n[Container]\nAutoUpdate=registry\n"
        );
        assert_eq!(
            set_autoupdate("[Volume]\nName=data\n", None),
            "[Volume]\nName=data\n"
        );
    }
}
