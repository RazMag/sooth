//! Version-looking image tags: whether a container is *pinned* (its
//! `Image=` names a tag like `1.25.3`, `v2.1` or `16.4-alpine3.20` rather
//! than a floating one like `latest`), and which of a registry's tags are
//! newer versions of the same kind of tag. Pure -- the registry listing is
//! `imageinfo::list_tags`, the schedule `crate::imagetags`.
//!
//! Two tags are only compared when they have the same *shape*: the same
//! leading letters (`v`, `version-`), the same number of dotted version
//! parts, and the same suffix once its digits are ignored -- so
//! `16.4-alpine3.20` can move to `16.5-alpine3.21` but never to
//! `17-bookworm`, and `2026.10.2` never to the floating `2026.10`.
//! Prerelease-looking tags (`1.2.0-rc1`, `2026.11.0b3`,
//! `2026.11.0.dev20261020`) are never versions here, so they're neither
//! "pinned" nor offered as an upgrade.

use std::cmp::Ordering;

/// `image` split into repository and tag -- `None` for a digest-pinned
/// reference or one with no tag at all (podman's implied `latest`). A port
/// in the registry part (`registry:5000/app`) isn't mistaken for a tag.
pub fn split_tag(image: &str) -> Option<(&str, &str)> {
    let image = image.trim();
    if image.contains('@') {
        return None;
    }
    let name_start = image.rfind('/').map_or(0, |i| i + 1);
    let colon = name_start + image[name_start..].rfind(':')?;
    let (repo, tag) = (&image[..colon], &image[colon + 1..]);
    (!repo.is_empty() && !tag.is_empty()).then_some((repo, tag))
}

/// Whether `image` is pinned to a version tag -- the containers sooth
/// looks for newer tags of.
pub fn is_pinned(image: &str) -> bool {
    split_tag(image).is_some_and(|(_, tag)| Version::parse(tag).is_some())
}

/// A parsed version tag. See the module docs for what counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    prefix: String,
    parts: Vec<u64>,
    /// The suffix with every run of digits replaced by `#`.
    shape: String,
    /// The suffix's digit runs, in order -- a build or base-image number
    /// (`-ls245`, `-alpine3.20`), compared after `parts`.
    suffix_nums: Vec<u64>,
}

/// Letter runs that mark a prerelease, matched case-insensitively against
/// each run of letters in a tag's suffix (`b` is Home Assistant's `0b3`).
const PRERELEASE: &[&str] = &[
    "a",
    "alpha",
    "b",
    "beta",
    "canary",
    "dev",
    "experimental",
    "nightly",
    "pre",
    "preview",
    "rc",
    "snapshot",
    "test",
    "unstable",
];

impl Version {
    pub fn parse(tag: &str) -> Option<Self> {
        let start = tag.find(|c: char| c.is_ascii_digit())?;
        let (prefix, rest) = tag.split_at(start);
        if !valid_prefix(prefix) {
            return None;
        }
        let mut parts = Vec::new();
        let mut i = 0;
        loop {
            let run = rest[i..]
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len() - i);
            parts.push(rest[i..i + run].parse().ok()?);
            i += run;
            let more = rest[i..].strip_prefix('.');
            if more.is_some_and(|m| m.starts_with(|c: char| c.is_ascii_digit())) {
                i += 1;
            } else {
                break;
            }
        }
        let suffix = &rest[i..];
        if !suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
            || is_prerelease(suffix)
        {
            return None;
        }
        let (shape, suffix_nums) = split_digits(suffix)?;
        Some(Self {
            prefix: prefix.to_string(),
            parts,
            shape,
            suffix_nums,
        })
    }

    fn same_shape(&self, other: &Self) -> bool {
        self.prefix == other.prefix
            && self.parts.len() == other.parts.len()
            && self.shape == other.shape
    }

    /// Orders two tags of the same shape.
    fn cmp_key(&self, other: &Self) -> Ordering {
        (&self.parts, &self.suffix_nums).cmp(&(&other.parts, &other.suffix_nums))
    }
}

/// Nothing, or a few letters optionally ending in `-`/`_` (`v`,
/// `version-`, `release_`).
fn valid_prefix(prefix: &str) -> bool {
    let letters = prefix.trim_end_matches(['-', '_']);
    prefix.len() <= 16
        && prefix.len() - letters.len() <= 1
        && letters.chars().all(|c| c.is_ascii_alphabetic())
        && (letters.is_empty() || !PRERELEASE.contains(&letters.to_ascii_lowercase().as_str()))
}

fn is_prerelease(suffix: &str) -> bool {
    suffix
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|run| PRERELEASE.contains(&run.to_ascii_lowercase().as_str()))
}

/// `suffix` with its digit runs replaced by `#`, plus those runs parsed --
/// `None` if one overflows.
fn split_digits(suffix: &str) -> Option<(String, Vec<u64>)> {
    let mut shape = String::new();
    let mut nums = Vec::new();
    let mut digits = String::new();
    for c in suffix.chars().chain(std::iter::once('\0')) {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if !digits.is_empty() {
            nums.push(digits.parse().ok()?);
            digits.clear();
            shape.push('#');
        }
        if c != '\0' {
            shape.push(c);
        }
    }
    Some((shape, nums))
}

/// One newer tag worth offering, with the range it's the newest of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub tag: String,
    /// `"2026.10.x"`-style for "newest within this version line", `None`
    /// for the newest overall.
    pub within: Option<String>,
}

/// The newer tags of `current`'s shape among `tags`, narrowest range first:
/// the newest sharing all but the last version part (the latest patch),
/// then all but the last two, ..., then the newest overall -- each tag
/// listed once, under its narrowest range. Empty when `current` isn't a
/// version tag or nothing newer exists.
pub fn newer(current: &str, tags: &[String]) -> Vec<Candidate> {
    let Some(cur) = Version::parse(current) else {
        return Vec::new();
    };
    let newer: Vec<(Version, &str)> = tags
        .iter()
        .filter_map(|t| Some((Version::parse(t)?, t.as_str())))
        .filter(|(v, _)| v.same_shape(&cur) && v.cmp_key(&cur) == Ordering::Greater)
        .collect();
    let best = |keep: usize| {
        newer
            .iter()
            .filter(|(v, _)| v.parts[..keep] == cur.parts[..keep])
            .max_by(|(a, _), (b, _)| a.cmp_key(b))
            .map(|(_, t)| *t)
    };
    let mut out: Vec<Candidate> = Vec::new();
    let scopes = (1..cur.parts.len()).rev().map(Some).chain([None]);
    for keep in scopes {
        let Some(tag) = best(keep.unwrap_or(0)) else {
            continue;
        };
        if out.iter().any(|c| c.tag == tag) {
            continue;
        }
        let within = keep.map(|k| {
            let line: Vec<String> = cur.parts[..k].iter().map(u64::to_string).collect();
            format!("{}{}.x", cur.prefix, line.join("."))
        });
        out.push(Candidate {
            tag: tag.to_string(),
            within,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    fn found(current: &str, all: &[&str]) -> Vec<(String, Option<String>)> {
        newer(current, &tags(all))
            .into_iter()
            .map(|c| (c.tag, c.within))
            .collect()
    }

    #[test]
    fn splits_repository_and_tag() {
        assert_eq!(split_tag("nginx:1.25"), Some(("nginx", "1.25")));
        assert_eq!(
            split_tag("registry:5000/app:2.0"),
            Some(("registry:5000/app", "2.0"))
        );
        assert_eq!(split_tag("registry:5000/app"), None);
        assert_eq!(split_tag("nginx"), None);
        assert_eq!(split_tag("nginx:1.25@sha256:abc"), None);
        assert_eq!(split_tag("nginx:"), None);
    }

    #[test]
    fn recognises_pinned_tags() {
        for image in [
            "nginx:1.25.3",
            "ghcr.io/home-assistant/home-assistant:2026.10.2",
            "postgres:16-alpine",
            "app:v2.1",
            "lscr.io/linuxserver/sonarr:4.0.9.2244-ls257",
            "app:version-1.2.3",
        ] {
            assert!(is_pinned(image), "{image}");
        }
        for image in [
            "nginx",
            "nginx:latest",
            "nginx:alpine",
            "ghcr.io/home-assistant/home-assistant:stable",
            "ghcr.io/home-assistant/home-assistant:2026.11.0b3",
            "ghcr.io/home-assistant/home-assistant:2026.11.0.dev20261020",
            "app:1.0.0-rc1",
            "app:2.0-beta.2",
            "app:1.2@sha256:abc",
        ] {
            assert!(!is_pinned(image), "{image}");
        }
    }

    #[test]
    fn home_assistant_calver() {
        let all = [
            "2026.9.1",
            "2026.9.3",
            "2026.9.4",
            "2026.10.0",
            "2026.10.2",
            "2026.10",
            "2026.11.0b3",
            "2026.11.0.dev20261020",
            "stable",
            "sha256-ff875078181a0383bf3fca9f061be12d6180896dbd.sig",
        ];
        assert_eq!(
            found("2026.9.3", &all),
            [
                ("2026.9.4".into(), Some("2026.9.x".into())),
                ("2026.10.2".into(), Some("2026.x".into())),
            ]
        );
        // Numeric, not lexical: 2026.10 is newer than 2026.9.
        assert_eq!(found("2026.10.2", &all), []);
        assert_eq!(
            found("2026.9", &all),
            [("2026.10".into(), Some("2026.x".into()))]
        );
    }

    #[test]
    fn ranges_narrowest_first_and_deduplicated() {
        let all = ["1.2.3", "1.2.9", "1.4.0", "2.0.1", "3.1.0"];
        assert_eq!(
            found("1.2.3", &all),
            [
                ("1.2.9".into(), Some("1.2.x".into())),
                ("1.4.0".into(), Some("1.x".into())),
                ("3.1.0".into(), None),
            ]
        );
        // The newest patch is also the newest overall: listed once.
        assert_eq!(
            found("3.0.0", &["3.0.1"]),
            [("3.0.1".into(), Some("3.0.x".into()))]
        );
        // A single part has only the overall range.
        assert_eq!(
            found("16", &["15", "16", "17", "17.1"]),
            [("17".into(), None)]
        );
    }

    #[test]
    fn only_compares_the_same_shape() {
        let all = [
            "16.4-alpine3.20",
            "16.5-alpine3.21",
            "16.5-bookworm",
            "17-alpine",
            "17.0",
            "v16.9-alpine3.21",
        ];
        assert_eq!(
            found("16.4-alpine3.20", &all),
            [("16.5-alpine3.21".into(), Some("16.x".into()))]
        );
        assert_eq!(
            found("v1.2", &["1.9", "v1.3", "v2.0"]),
            [("v1.3".into(), Some("v1.x".into())), ("v2.0".into(), None)]
        );
    }

    #[test]
    fn suffix_build_numbers_count_after_the_version() {
        let all = [
            "4.0.9.2244-ls257",
            "4.0.9.2244-ls260",
            "4.0.10.2544-ls261",
            "4.0.9.2244",
        ];
        assert_eq!(
            found("4.0.9.2244-ls257", &all),
            [
                ("4.0.9.2244-ls260".into(), Some("4.0.9.x".into())),
                ("4.0.10.2544-ls261".into(), Some("4.0.x".into())),
            ]
        );
    }

    #[test]
    fn nothing_for_floating_or_prerelease_tags() {
        assert!(newer("latest", &tags(&["1.0", "2.0"])).is_empty());
        assert!(newer("1.0.0-rc1", &tags(&["1.0.0-rc2", "1.0.1-rc1"])).is_empty());
        assert!(newer("1.0", &tags(&["1.1-rc1", "1.1b1"])).is_empty());
    }
}
