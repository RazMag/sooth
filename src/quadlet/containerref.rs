//! Patches a `.container` quadlet's `[Container]` `Pod=` line -- the reverse
//! side of Quadlet's pod-membership model, where a container opts itself into
//! a pod rather than being listed by the pod. Used when the "New Pod" page
//! attaches an already-existing container to a freshly created pod.

/// Ensures the `[Container]` section carries exactly one `Pod=<value>` line,
/// replacing any existing `Pod=` value -- a container belongs to at most one
/// pod, unlike `EnvironmentFile=` (see `envfile::patch_environment_file`),
/// which can legitimately repeat with distinct values and so is only ever
/// ensured-present, never rewritten in place. Removes the line entirely when
/// `value` is `None`. Patches `raw` as text, consistent with every other edit
/// in this app; every other line is left byte-for-byte alone.
pub fn set_pod(raw: &str, value: Option<&str>) -> String {
    let had_trailing_nl = raw.ends_with('\n');
    let mut lines: Vec<String> = raw.split('\n').map(str::to_string).collect();
    if had_trailing_nl {
        lines.pop(); // the empty element `split` leaves after a final '\n'
    }

    let is_any_header = |l: &str| {
        let t = l.trim();
        t.starts_with('[') && t.ends_with(']')
    };
    let opens_container = |l: &str| {
        l.trim()
            .strip_prefix('[')
            .and_then(|x| x.strip_suffix(']'))
            .is_some_and(|name| name.trim().eq_ignore_ascii_case("Container"))
    };
    let is_pod_line = |l: &str| l.trim_start().starts_with("Pod=");

    let finish = |lines: Vec<String>| {
        let mut out = lines.join("\n");
        if had_trailing_nl {
            out.push('\n');
        }
        out
    };

    let Some(header_idx) = lines.iter().position(|l| opens_container(l)) else {
        // The [Container] section is always present in practice (create/edit
        // run `writer::validate` first). This branch is a defensive fallback.
        if let Some(v) = value {
            if lines.iter().any(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[Container]".to_string());
            lines.push(format!("Pod={v}"));
        }
        return finish(lines);
    };

    let body_start = header_idx + 1;
    let body_end = lines[body_start..]
        .iter()
        .position(|l| is_any_header(l))
        .map_or(lines.len(), |p| body_start + p);

    let pod_idx: Vec<usize> = (body_start..body_end)
        .filter(|&i| is_pod_line(&lines[i]))
        .collect();

    match value {
        Some(v) => {
            let managed = format!("Pod={v}");
            if let Some(&first) = pod_idx.first() {
                lines[first] = managed;
                for &i in pod_idx[1..].iter().rev() {
                    lines.remove(i);
                }
            } else {
                let mut ins = body_end;
                while ins > body_start && lines[ins - 1].trim().is_empty() {
                    ins -= 1;
                }
                lines.insert(ins, managed);
            }
        }
        None => {
            for &i in pod_idx.iter().rev() {
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
    fn inserts_at_end_of_section_when_absent() {
        let raw = "[Unit]\nDescription=x\n\n[Container]\nImage=alpine\n";
        let out = set_pod(raw, Some("app.pod"));
        assert_eq!(
            out,
            "[Unit]\nDescription=x\n\n[Container]\nImage=alpine\nPod=app.pod\n"
        );
    }

    #[test]
    fn inserts_before_trailing_blank_lines() {
        let raw = "[Container]\nImage=alpine\n\n\n";
        let out = set_pod(raw, Some("app.pod"));
        assert_eq!(out, "[Container]\nImage=alpine\nPod=app.pod\n\n\n");
    }

    #[test]
    fn replaces_an_existing_value_in_place() {
        let raw = "[Container]\nImage=alpine\nPod=old.pod\nEnvironmentFile=x.env\n";
        let out = set_pod(raw, Some("new.pod"));
        assert_eq!(
            out,
            "[Container]\nImage=alpine\nPod=new.pod\nEnvironmentFile=x.env\n"
        );
    }

    #[test]
    fn unchanged_when_already_the_requested_value() {
        let raw = "[Container]\nImage=alpine\nPod=app.pod\n";
        assert_eq!(set_pod(raw, Some("app.pod")), raw);
    }

    #[test]
    fn collapses_duplicate_pod_lines_to_the_new_value() {
        let raw = "[Container]\nImage=alpine\nPod=a.pod\nKey=v\nPod=b.pod\n";
        let out = set_pod(raw, Some("c.pod"));
        assert_eq!(out, "[Container]\nImage=alpine\nPod=c.pod\nKey=v\n");
    }

    #[test]
    fn none_removes_the_line() {
        let raw = "[Container]\nImage=alpine\nPod=app.pod\n\n[Install]\nWantedBy=default.target\n";
        let out = set_pod(raw, None);
        assert_eq!(
            out,
            "[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\n"
        );
    }

    #[test]
    fn none_with_nothing_to_remove_is_unchanged() {
        let raw = "[Container]\nImage=alpine\n";
        assert_eq!(set_pod(raw, None), raw);
    }

    #[test]
    fn matches_header_case_insensitively_and_with_trailing_space() {
        let raw = "[container] \nImage=alpine\n";
        let out = set_pod(raw, Some("app.pod"));
        assert_eq!(out, "[container] \nImage=alpine\nPod=app.pod\n");
    }

    #[test]
    fn leaves_other_sections_and_keys_untouched() {
        let raw = "[Unit]\nPod=not-this-one\n\n[Container]\nImage=alpine\n\n[Install]\nWantedBy=default.target\n";
        let out = set_pod(raw, Some("app.pod"));
        assert_eq!(
            out,
            "[Unit]\nPod=not-this-one\n\n[Container]\nImage=alpine\nPod=app.pod\n\n[Install]\nWantedBy=default.target\n"
        );
    }
}
