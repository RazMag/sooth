//! The quadlet `[Install]` section -- the rootless equivalent of
//! `systemctl enable`.
//!
//! Podman writes each quadlet's `.service` unit into
//! `/run/user/<uid>/systemd/generator/`, and systemd refuses
//! `EnableUnitFiles` / `DisableUnitFiles` on anything under a generator
//! directory ("Unit ... is transient or generated"). The quadlet generator
//! instead reads an `[Install]` section straight out of the source file and
//! writes the `default.target.wants/` symlink itself. So "enable" / "disable"
//! for a sooth-managed unit is a text patch of that section followed by a
//! daemon reload -- never a D-Bus enable call.
//!
//! Consistent with the rest of sooth, [`set_enabled`] patches the raw file
//! text rather than re-serialising the parsed model, so comments and
//! formatting the app didn't touch survive byte-for-byte.
//!
//! Whether a unit is *currently* enabled is not read back from here -- the
//! file can name a `WantedBy=` target that doesn't exist or isn't in the
//! login path. That question is answered by systemd's computed reverse deps:
//! `UnitStatus::is_autostart_enabled` (see `systemd::status`).

/// The one target this module adds or removes. `default.target` is the
/// rootless user-session equivalent of `multi-user.target` -- it's what a
/// `systemctl --user` login reaches, so a unit wanted by it starts on login.
/// Any *other* target the user put in `[Install]` is left strictly alone.
const LOGIN_TARGET: &str = "default.target";
const MANAGED_LINE: &str = "WantedBy=default.target";
const SECTION: &str = "Install";

fn is_install_target(key: &str) -> bool {
    let k = key.trim();
    k.eq_ignore_ascii_case("WantedBy") || k.eq_ignore_ascii_case("RequiredBy")
}

/// `Some((key, targets))` when `line` is an `[Install]` `WantedBy=` /
/// `RequiredBy=` assignment, with the value split into its whitespace-
/// separated target list (systemd allows `WantedBy=a.target b.target`).
/// `None` for anything else.
fn install_target_line(line: &str) -> Option<(&str, Vec<&str>)> {
    let (k, v) = line.trim().split_once('=')?;
    is_install_target(k).then(|| (k.trim(), v.split_whitespace().collect()))
}

/// Add (`enabled == true`) or remove (`enabled == false`) the managed
/// `WantedBy=default.target` in `raw`'s `[Install]` section, leaving every
/// other line -- including any other `WantedBy=` / `RequiredBy=` target the
/// user wrote -- byte-for-byte alone. Text already in the desired state is
/// returned unchanged.
///
/// * enable: if some `WantedBy=` / `RequiredBy=` in the `[Install]` section
///   already lists `default.target`, nothing changes; otherwise
///   `WantedBy=default.target` is appended -- to the existing section, or a
///   fresh one at end of file. A populated `WantedBy=` naming only other
///   targets does *not* count as enabled, so this adds the login target
///   alongside it.
/// * disable: `default.target` is dropped from every `WantedBy=` /
///   `RequiredBy=` that lists it -- the whole line if that leaves it empty,
///   and the section header too if that empties the section. A hand-written
///   `Alias=`, or a `WantedBy=` naming only other targets, keeps it alive.
///
/// Round-trips LF text; on a CRLF file the `\r` stays attached to untouched
/// lines (matching is on the trimmed line) but a newly inserted line is
/// bare-LF, mirroring `envfile::patch_environment_file`.
pub fn set_enabled(raw: &str, enabled: bool) -> String {
    let had_trailing_nl = raw.ends_with('\n');
    let mut lines: Vec<String> = raw.split('\n').map(str::to_string).collect();
    if had_trailing_nl {
        lines.pop(); // the empty element `split` leaves after a final '\n'
    }

    let is_any_header = |l: &str| {
        let t = l.trim();
        t.starts_with('[') && t.ends_with(']')
    };
    let opens_install = |l: &str| {
        l.trim()
            .strip_prefix('[')
            .and_then(|x| x.strip_suffix(']'))
            .is_some_and(|name| name.trim().eq_ignore_ascii_case(SECTION))
    };
    let names_login = |l: &str| {
        install_target_line(l).is_some_and(|(_, targets)| targets.contains(&LOGIN_TARGET))
    };
    let is_blank_or_comment = |l: &str| {
        let t = l.trim();
        t.is_empty() || t.starts_with('#') || t.starts_with(';')
    };

    let finish = |lines: Vec<String>| {
        let mut out = lines.join("\n");
        if had_trailing_nl {
            out.push('\n');
        }
        out
    };

    let header_idx = lines.iter().position(|l| opens_install(l));

    if enabled {
        match header_idx {
            Some(h) => {
                let body_start = h + 1;
                let body_end = lines[body_start..]
                    .iter()
                    .position(|l| is_any_header(l))
                    .map_or(lines.len(), |p| body_start + p);
                if (body_start..body_end).any(|i| names_login(&lines[i])) {
                    return finish(lines); // already wanted by default.target
                }
                let mut ins = body_end;
                while ins > body_start && lines[ins - 1].trim().is_empty() {
                    ins -= 1;
                }
                lines.insert(ins, MANAGED_LINE.to_string());
            }
            None => {
                if lines.iter().any(|l| !l.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("[{SECTION}]"));
                lines.push(MANAGED_LINE.to_string());
            }
        }
        return finish(lines);
    }

    // disable: drop `default.target` wherever it's listed, but leave any
    // other target (a hand-written `multi-user.target`, a `RequiredBy=`) as
    // the user wrote it.
    let Some(h) = header_idx else {
        return finish(lines); // no [Install] section -- already disabled
    };
    let body_start = h + 1;
    let mut body_end = lines[body_start..]
        .iter()
        .position(|l| is_any_header(l))
        .map_or(lines.len(), |p| body_start + p);

    enum Act {
        Keep,
        Drop,
        Rewrite(String),
    }
    for i in (body_start..body_end).rev() {
        let act = match install_target_line(&lines[i]) {
            Some((key, targets)) if targets.contains(&LOGIN_TARGET) => {
                let rest: Vec<&str> =
                    targets.iter().copied().filter(|t| *t != LOGIN_TARGET).collect();
                if rest.is_empty() {
                    Act::Drop
                } else {
                    Act::Rewrite(format!("{key}={}", rest.join(" ")))
                }
            }
            _ => Act::Keep,
        };
        match act {
            Act::Keep => {}
            Act::Drop => {
                lines.remove(i);
                body_end -= 1;
            }
            Act::Rewrite(s) => lines[i] = s,
        }
    }

    // Nothing but blanks/comments left under the header -> drop the whole
    // section, then collapse the separator/trailing blanks it leaves behind.
    if (body_start..body_end).all(|i| is_blank_or_comment(&lines[i])) {
        lines.drain(h..body_end);
        while h > 0
            && lines.get(h - 1).is_some_and(|l| l.trim().is_empty())
            && lines.get(h).is_none_or(|l| l.trim().is_empty())
        {
            lines.remove(h - 1);
        }
    }

    finish(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_appends_a_fresh_section_at_eof() {
        let raw = "[Container]\nImage=alpine\n";
        assert_eq!(
            set_enabled(raw, true),
            "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\n"
        );
    }

    #[test]
    fn enable_is_idempotent_when_already_wanted() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\n";
        assert_eq!(set_enabled(raw, true), raw);
    }

    #[test]
    fn enable_adds_the_login_target_next_to_a_hand_written_one() {
        // A populated `WantedBy=` that isn't `default.target` does not make
        // the unit autostart on login, so Enable appends the login target
        // rather than treating the section as already enabled.
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=multi-user.target\n";
        assert_eq!(
            set_enabled(raw, true),
            "[Container]\nImage=alpine\n\n[Install]\nWantedBy=multi-user.target\nWantedBy=default.target\n"
        );
    }

    #[test]
    fn enable_is_idempotent_when_login_target_shares_a_line() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=other.target default.target\n";
        assert_eq!(set_enabled(raw, true), raw);
    }

    #[test]
    fn enable_extends_an_install_section_that_has_no_target() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nAlias=web.service\n";
        assert_eq!(
            set_enabled(raw, true),
            "[Container]\nImage=alpine\n\n[Install]\nAlias=web.service\nWantedBy=default.target\n"
        );
    }

    #[test]
    fn disable_removes_the_section_it_owns_mid_file() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\n\n[Service]\nRestart=always\n";
        assert_eq!(
            set_enabled(raw, false),
            "[Container]\nImage=alpine\n\n[Service]\nRestart=always\n"
        );
    }

    #[test]
    fn disable_removes_a_trailing_install_section_and_its_blank_gap() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\n";
        assert_eq!(set_enabled(raw, false), "[Container]\nImage=alpine\n");
    }

    #[test]
    fn disable_removes_only_the_login_target_not_other_deps() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\nRequiredBy=x.target\n";
        assert_eq!(
            set_enabled(raw, false),
            "[Container]\nImage=alpine\n\n[Install]\nRequiredBy=x.target\n"
        );
    }

    #[test]
    fn disable_strips_the_login_target_from_a_shared_line() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target other.target\n";
        assert_eq!(
            set_enabled(raw, false),
            "[Container]\nImage=alpine\n\n[Install]\nWantedBy=other.target\n"
        );
    }

    #[test]
    fn disable_is_a_noop_when_only_other_targets_are_present() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=multi-user.target\n";
        assert_eq!(set_enabled(raw, false), raw);
    }

    #[test]
    fn disable_keeps_a_section_with_other_keys() {
        let raw =
            "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\nAlias=web.service\n";
        assert_eq!(
            set_enabled(raw, false),
            "[Container]\nImage=alpine\n\n[Install]\nAlias=web.service\n"
        );
    }

    #[test]
    fn disable_is_a_noop_without_an_install_section() {
        let raw = "[Container]\nImage=alpine\n";
        assert_eq!(set_enabled(raw, false), raw);
    }

    #[test]
    fn disable_then_enable_round_trips_to_a_canonical_section() {
        let raw = "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\n";
        let off = set_enabled(raw, false);
        assert_eq!(off, "[Container]\nImage=alpine\n");
        assert_eq!(set_enabled(&off, true), raw);
    }

    #[test]
    fn header_match_tolerates_case_and_trailing_space() {
        let raw = "[Container]\nImage=alpine\n\n[install] \nWantedBy=default.target\n";
        assert_eq!(set_enabled(raw, false), "[Container]\nImage=alpine\n");
    }

    #[test]
    fn preserves_a_file_with_no_trailing_newline() {
        let raw = "[Container]\nImage=alpine";
        assert_eq!(
            set_enabled(raw, true),
            "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target"
        );
    }
}
