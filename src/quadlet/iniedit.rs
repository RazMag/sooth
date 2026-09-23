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
    let (mut lines, had_trailing_nl) = split_lines(raw);

    let Some(header_idx) = lines.iter().position(|l| opens_section(l, section)) else {
        // The primary section is always present in practice (create/edit run
        // `writer::validate` first). This branch is a defensive fallback.
        if present {
            if lines.iter().any(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{section}]"));
            lines.push(managed);
        }
        return join_lines(lines, had_trailing_nl);
    };

    let body = section_body(&lines, header_idx);
    let managed_idx: Vec<usize> = body
        .clone()
        .filter(|&i| lines[i].trim() == managed)
        .collect();

    if present {
        if managed_idx.is_empty() {
            let ins = insert_pos(&lines, body);
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

    join_lines(lines, had_trailing_nl)
}

/// Splits `raw` on `\n` into a mutable line vector, dropping the empty
/// trailing element `split` leaves after a final newline, alongside whether
/// that trailing newline was present. Pair with [`join_lines`] to
/// round-trip; shared scaffolding for [`patch_line`] and the sibling
/// raw-text patchers in [`super::install`] and [`super::autoupdate`], whose
/// matching/replace semantics differ enough that they can't just call
/// `patch_line` itself.
pub(crate) fn split_lines(raw: &str) -> (Vec<String>, bool) {
    let had_trailing_nl = raw.ends_with('\n');
    let mut lines: Vec<String> = raw.split('\n').map(str::to_string).collect();
    if had_trailing_nl {
        lines.pop();
    }
    (lines, had_trailing_nl)
}

/// Inverse of [`split_lines`]: rejoins `lines` and restores the trailing
/// newline if the original had one.
pub(crate) fn join_lines(lines: Vec<String>, had_trailing_nl: bool) -> String {
    let mut out = lines.join("\n");
    if had_trailing_nl {
        out.push('\n');
    }
    out
}

/// True when `l` is any `[section]` header line.
pub(crate) fn is_any_header(l: &str) -> bool {
    let t = l.trim();
    t.starts_with('[') && t.ends_with(']')
}

/// True when `l` is a `[section]` header naming `section` specifically
/// (case-insensitive, leading/trailing whitespace on the name tolerated).
pub(crate) fn opens_section(l: &str, section: &str) -> bool {
    l.trim()
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .is_some_and(|name| name.trim().eq_ignore_ascii_case(section))
}

/// The `[body_start, body_end)` line-index range owned by the section whose
/// header is at `header_idx` -- from just after the header to the next
/// header line (any section) or EOF.
pub(crate) fn section_body(lines: &[String], header_idx: usize) -> std::ops::Range<usize> {
    let body_start = header_idx + 1;
    let body_end = lines[body_start..]
        .iter()
        .position(|l| is_any_header(l))
        .map_or(lines.len(), |p| body_start + p);
    body_start..body_end
}

/// The index within `body` to insert a new key line at, so it lands after
/// the section's existing keys but before any trailing blank separator
/// lines at the end of the section.
pub(crate) fn insert_pos(lines: &[String], body: std::ops::Range<usize>) -> usize {
    let mut ins = body.end;
    while ins > body.start && lines[ins - 1].trim().is_empty() {
        ins -= 1;
    }
    ins
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
