//! Generic raw-text `[section]` line patching: ensure a repeatable
//! `key=value` line is present (or gone), leaving comments, formatting, and
//! every other line -- including other values of the same repeatable key --
//! untouched. `envfile::patch_environment_file` and the "New Pod"/"Edit Pod"
//! pages' network/volume pickers (`Network=`/`Volume=` lines added to a
//! Pod's own `[Pod]` section) both build on this. `containerref::set_pod`
//! doesn't: `Pod=` is a singleton key it replaces rather than appends
//! alongside.

/// See the module doc comment.
///
/// * `present == true`  -- guarantee one `key=value` line exists (inserted
///   at the end of the section's key lines, before any trailing blank
///   line); duplicates of the exact same line collapse to the first.
/// * `present == false` -- remove every line matching `key=value` exactly.
///
/// Text already in the desired state is returned unchanged. Consistent with
/// the rest of sooth, this patches `raw` as text rather than re-serialising
/// the parsed model.
///
/// Round-trips LF text; on a CRLF file the `\r` stays attached to untouched
/// lines and matching still works (comparison is on the trimmed line), but a
/// newly inserted line is bare-LF.
pub fn patch_line(raw: &str, section: &str, key: &str, value: &str, present: bool) -> String {
    let managed = format!("{key}={value}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_at_end_of_section() {
        let raw = "[Unit]\nDescription=x\n\n[Pod]\nPublishPort=80:80\n";
        let out = patch_line(raw, "Pod", "Network", "frontend.network", true);
        assert_eq!(
            out,
            "[Unit]\nDescription=x\n\n[Pod]\nPublishPort=80:80\nNetwork=frontend.network\n"
        );
    }

    #[test]
    fn inserts_before_trailing_blank_lines() {
        let raw = "[Pod]\nPublishPort=80:80\n\n\n";
        let out = patch_line(raw, "Pod", "Network", "frontend.network", true);
        assert_eq!(
            out,
            "[Pod]\nPublishPort=80:80\nNetwork=frontend.network\n\n\n"
        );
    }

    #[test]
    fn leaves_other_values_of_the_same_repeatable_key_alone() {
        let raw = "[Pod]\nNetwork=a.network\n";
        let out = patch_line(raw, "Pod", "Network", "b.network", true);
        assert_eq!(out, "[Pod]\nNetwork=a.network\nNetwork=b.network\n");
    }

    #[test]
    fn is_idempotent_for_an_exact_duplicate() {
        let raw = "[Pod]\nNetwork=a.network\n";
        assert_eq!(patch_line(raw, "Pod", "Network", "a.network", true), raw);
    }

    #[test]
    fn collapses_exact_duplicates_to_the_first() {
        let raw = "[Pod]\nNetwork=a.network\nPublishPort=80:80\nNetwork=a.network\n";
        let out = patch_line(raw, "Pod", "Network", "a.network", true);
        assert_eq!(out, "[Pod]\nNetwork=a.network\nPublishPort=80:80\n");
    }

    #[test]
    fn removes_only_the_exact_match() {
        let raw = "[Pod]\nVolume=a.volume:/a\nVolume=b.volume:/b\n";
        let out = patch_line(raw, "Pod", "Volume", "a.volume:/a", false);
        assert_eq!(out, "[Pod]\nVolume=b.volume:/b\n");
    }

    #[test]
    fn removal_with_nothing_to_remove_is_unchanged() {
        let raw = "[Pod]\nNetwork=a.network\n";
        assert_eq!(patch_line(raw, "Pod", "Network", "b.network", false), raw);
    }

    #[test]
    fn creates_a_missing_section_at_eof() {
        let raw = "[Unit]\nDescription=x\n";
        let out = patch_line(raw, "Pod", "Network", "a.network", true);
        assert_eq!(out, "[Unit]\nDescription=x\n\n[Pod]\nNetwork=a.network\n");
    }

    #[test]
    fn matches_header_case_insensitively_and_with_trailing_space() {
        let raw = "[pod] \nPublishPort=80:80\n";
        let out = patch_line(raw, "Pod", "Network", "a.network", true);
        assert_eq!(out, "[pod] \nPublishPort=80:80\nNetwork=a.network\n");
    }
}
