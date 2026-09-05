use super::model::Section;
use super::QuadletError;

/// Parses a quadlet/systemd-unit-style INI file into ordered sections,
/// preserving duplicate keys. Comments (`#`/`;`) and blank lines are dropped
/// from the structural model -- the original text is kept separately
/// (`QuadletUnit::raw`) as the basis for any rewrite, so this parser only has
/// to answer "what keys/values does this file declare", never "reproduce
/// this file byte for byte".
///
/// Known limitation (documented, not silently wrong): systemd unit files
/// support a trailing-backslash line continuation; quadlet files in the wild
/// rarely need it, and it is not implemented here for v1.
pub fn parse(text: &str) -> Result<Vec<Section>, QuadletError> {
    let mut sections: Vec<Section> = Vec::new();
    let mut current: Option<Section> = None;

    for (lineno, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            current = Some(Section { name: header.trim().to_string(), entries: Vec::new() });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(QuadletError::Validation(format!(
                "line {}: expected `Key=Value` or `[Section]`, found: {line}",
                lineno + 1
            )));
        };
        let Some(section) = current.as_mut() else {
            return Err(QuadletError::Validation(format!(
                "line {}: key `{key}` appears before any `[Section]` header",
                lineno + 1
            )));
        };
        section.entries.push((key.trim().to_string(), value.trim().to_string()));
    }
    if let Some(section) = current.take() {
        sections.push(section);
    }
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_and_preserves_duplicates() {
        let text = "\
[Unit]
Description=demo

[Container]
Image=docker.io/library/alpine
Volume=/a:/a
Volume=/b:/b
Environment=FOO=bar
";
        let sections = parse(text).unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "Unit");
        assert_eq!(sections[1].name, "Container");
        let volumes: Vec<_> =
            sections[1].entries.iter().filter(|(k, _)| k == "Volume").map(|(_, v)| v.as_str()).collect();
        assert_eq!(volumes, vec!["/a:/a", "/b:/b"]);
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let text = "\
# a comment
; another comment

[Container]
Image=alpine
";
        let sections = parse(text).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].get("Image"), Some("alpine"));
    }

    #[test]
    fn rejects_key_before_any_section() {
        let text = "Image=alpine\n[Container]\n";
        assert!(parse(text).is_err());
    }

    #[test]
    fn rejects_malformed_line() {
        let text = "[Container]\nnot-a-key-value-line\n";
        assert!(parse(text).is_err());
    }
}
