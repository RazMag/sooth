//! Parses `PublishPort=` declarations out of Container/Pod quadlet units and
//! detects host-port collisions between them. Pure and unit-testable: no
//! D-Bus, no filesystem, just `&[QuadletUnit]` in, structured data out.

use super::model::{QuadletUnit, UnitKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        }
    }
}

/// One `PublishPort=` entry, resolved to a host port range where the host
/// port is static (podman assigns dynamic ones at random -- those parse to
/// `host_port: None`, see `PortMapping::host_port`).
#[derive(Debug, Clone)]
pub struct PortMapping {
    pub file_name: String,
    /// `None` when podman assigns the host port randomly (the quadlet only
    /// specified a container port, e.g. `PublishPort=80` or `PublishPort=ip::80`).
    pub host_port: Option<HostPortRange>,
    pub protocol: Protocol,
    pub container_port: String,
    /// The original `PublishPort=` value, kept for display.
    pub raw: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostPortRange {
    pub start: u16,
    pub end: u16, // == start when not a range
}

impl HostPortRange {
    fn overlaps(self, other: Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// Parses every `PublishPort=` in the primary section of every Container or
/// Pod unit in `units`. Unparsable entries are skipped, never fatal -- a
/// typo'd port line shouldn't take down the whole ports screen.
pub fn extract(units: &[QuadletUnit]) -> Vec<PortMapping> {
    let mut out = Vec::new();
    for unit in units {
        if !matches!(unit.kind, UnitKind::Container | UnitKind::Pod) {
            continue;
        }
        let Some(section) = unit.section(unit.kind.primary_section()) else {
            continue;
        };
        for (key, value) in &section.entries {
            if key != "PublishPort" {
                continue;
            }
            if let Some(mapping) = parse_publish_port(&unit.file_name, value) {
                out.push(mapping);
            }
        }
    }
    out
}

/// Grammar (per `podman run -p` / podman-systemd.unit(5)):
/// `[[ip:]host_port:]container_port[/protocol]`, where `host_port` may be a
/// range (`8000-8010`) and `ip` may be bracketed IPv6 (`[::1]`).
fn parse_publish_port(file_name: &str, raw: &str) -> Option<PortMapping> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }

    let (rest, protocol) = match value.rsplit_once('/') {
        Some((rest, "udp")) => (rest, Protocol::Udp),
        Some((rest, "tcp")) => (rest, Protocol::Tcp),
        Some(_) => return None, // unrecognized protocol suffix
        None => (value, Protocol::Tcp),
    };

    let segments = split_respecting_brackets(rest);
    let (host_part, container_part) = match segments.as_slice() {
        [container] => (None, *container),
        [host, container] => (Some(*host), *container),
        [_ip, host, container] => (Some(*host), *container),
        _ => return None,
    };

    // Validate the container port parses as a port (or port range) too --
    // `container_port` is kept as the original string for display, but a
    // non-numeric value here means the whole line is malformed.
    parse_port_range(container_part)?;

    let host_port = match host_part {
        None | Some("") => None,
        Some(host) => Some(parse_port_range(host)?),
    };

    Some(PortMapping {
        file_name: file_name.to_string(),
        host_port,
        protocol,
        container_port: container_part.to_string(),
        raw: raw.to_string(),
    })
}

/// Splits on `:` while treating a leading `[...]` (bracketed IPv6 host) as
/// one opaque segment, so `[::1]:8080:80` splits into `["[::1]", "8080",
/// "80"]` rather than being shredded by the colons inside the brackets.
fn split_respecting_brackets(s: &str) -> Vec<&str> {
    if let Some(rest) = s.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        let ip = &s[..=end + 1];
        let tail = &rest[end + 1..];
        let tail = tail.strip_prefix(':').unwrap_or(tail);
        let mut parts = vec![ip];
        parts.extend(tail.split(':'));
        return parts;
    }
    s.split(':').collect()
}

fn parse_port_range(s: &str) -> Option<HostPortRange> {
    match s.split_once('-') {
        Some((start, end)) => {
            let start: u16 = start.parse().ok()?;
            let end: u16 = end.parse().ok()?;
            if end < start {
                return None;
            }
            Some(HostPortRange { start, end })
        }
        None => {
            let port: u16 = s.parse().ok()?;
            Some(HostPortRange {
                start: port,
                end: port,
            })
        }
    }
}

/// A port a container says it serves -- a `ExposePort=` line in its quadlet
/// or an `EXPOSE` baked into its image. Informational only (podman publishes
/// nothing for it), but it's the one declared signal of which container in a
/// pod is meant to receive a pod-published port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExposedPort {
    pub range: HostPortRange,
    pub protocol: Protocol,
}

/// Parses `80`, `80/tcp` or `8000-8010/udp` -- the shape of both
/// `ExposePort=` values and an image's `Config.ExposedPorts` keys.
pub fn parse_exposed(s: &str) -> Option<ExposedPort> {
    let s = s.trim();
    let (port, protocol) = match s.rsplit_once('/') {
        Some((port, "tcp")) => (port, Protocol::Tcp),
        Some((port, "udp")) => (port, Protocol::Udp),
        Some(_) => return None,
        None => (s, Protocol::Tcp),
    };
    Some(ExposedPort {
        range: parse_port_range(port)?,
        protocol,
    })
}

/// Every `ExposePort=` in a Container unit's `[Container]` section.
/// Unparsable values are skipped. Empty for every other kind.
pub fn declared_exposed(unit: &QuadletUnit) -> Vec<ExposedPort> {
    if unit.kind != UnitKind::Container {
        return Vec::new();
    }
    unit.section("Container")
        .map(|s| {
            s.entries
                .iter()
                .filter(|(k, _)| k == "ExposePort")
                .filter_map(|(_, v)| parse_exposed(v))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether any of `exposed` covers `mapping`'s container port on the same
/// protocol -- i.e. the container claims to be what that port reaches.
pub fn serves(exposed: &[ExposedPort], mapping: &PortMapping) -> bool {
    let Some(target) = parse_port_range(&mapping.container_port) else {
        return false;
    };
    exposed
        .iter()
        .any(|e| e.protocol == mapping.protocol && e.range.overlaps(target))
}

/// File names of every *other* quadlet whose static host-port range overlaps
/// `mapping`'s on the same protocol -- the units it really conflicts with.
/// Empty for a dynamic host port. Sorted and deduplicated. Keyed per mapping
/// (not per file), so a unit with two ports is only flagged on the one that
/// actually collides.
///
/// Host IP is deliberately ignored: over-flagging a same-port-different-IP
/// bind is safer than missing a real conflict on a single-host rootless setup.
pub fn conflicts_with(mapping: &PortMapping, all: &[PortMapping]) -> Vec<String> {
    let Some(host_port) = mapping.host_port else {
        return Vec::new();
    };
    let mut out: Vec<String> = all
        .iter()
        .filter(|other| other.file_name != mapping.file_name && other.protocol == mapping.protocol)
        .filter(|other| other.host_port.is_some_and(|r| r.overlaps(host_port)))
        .map(|other| other.file_name.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(file: &str, raw: &str) -> Option<PortMapping> {
        parse_publish_port(file, raw)
    }

    #[test]
    fn plain_container_port_is_dynamic() {
        let m = mapping("a.container", "80").unwrap();
        assert_eq!(m.host_port, None);
        assert_eq!(m.container_port, "80");
        assert_eq!(m.protocol, Protocol::Tcp);
    }

    #[test]
    fn host_and_container_port() {
        let m = mapping("a.container", "8080:80").unwrap();
        assert_eq!(
            m.host_port,
            Some(HostPortRange {
                start: 8080,
                end: 8080
            })
        );
        assert_eq!(m.container_port, "80");
    }

    #[test]
    fn ip_host_and_container_port() {
        let m = mapping("a.container", "127.0.0.1:8080:80").unwrap();
        assert_eq!(
            m.host_port,
            Some(HostPortRange {
                start: 8080,
                end: 8080
            })
        );
        assert_eq!(m.container_port, "80");
    }

    #[test]
    fn udp_suffix() {
        let m = mapping("a.container", "53:53/udp").unwrap();
        assert_eq!(m.protocol, Protocol::Udp);
    }

    #[test]
    fn host_port_range() {
        let m = mapping("a.container", "8000-8010:8000-8010").unwrap();
        assert_eq!(
            m.host_port,
            Some(HostPortRange {
                start: 8000,
                end: 8010
            })
        );
    }

    #[test]
    fn ip_with_empty_host_port_is_dynamic() {
        let m = mapping("a.container", "127.0.0.1::80").unwrap();
        assert_eq!(m.host_port, None);
    }

    #[test]
    fn bracketed_ipv6_host() {
        let m = mapping("a.container", "[::1]:8080:80").unwrap();
        assert_eq!(
            m.host_port,
            Some(HostPortRange {
                start: 8080,
                end: 8080
            })
        );
        assert_eq!(m.container_port, "80");
    }

    #[test]
    fn malformed_is_skipped_not_panicking() {
        assert!(mapping("a.container", "").is_none());
        assert!(mapping("a.container", "not-a-port").is_none());
        assert!(mapping("a.container", "8080:80/sctp").is_none());
        assert!(mapping("a.container", "1:2:3:4:5").is_none());
    }

    #[test]
    fn detects_conflict_across_two_files() {
        let mappings = vec![
            mapping("a.container", "8080:80").unwrap(),
            mapping("b.container", "8080-8090:8081").unwrap(),
            mapping("c.container", "9090:90").unwrap(),
            mapping("d.pod", "8085:80").unwrap(),
        ];
        assert_eq!(conflicts_with(&mappings[0], &mappings), vec!["b.container"]);
        assert_eq!(
            conflicts_with(&mappings[1], &mappings),
            vec!["a.container", "d.pod"]
        );
        assert!(conflicts_with(&mappings[2], &mappings).is_empty());
    }

    #[test]
    fn only_the_colliding_port_of_a_file_is_flagged() {
        let mappings = vec![
            mapping("a.container", "8080:80").unwrap(),
            mapping("a.container", "9000:90").unwrap(),
            mapping("b.container", "8080:80").unwrap(),
        ];
        assert_eq!(conflicts_with(&mappings[0], &mappings), vec!["b.container"]);
        assert!(conflicts_with(&mappings[1], &mappings).is_empty());
    }

    #[test]
    fn no_conflict_for_same_file_or_different_protocol() {
        let mappings = vec![
            mapping("a.container", "8080:80").unwrap(),
            mapping("a.container", "8080:81").unwrap(),
            mapping("b.container", "8080:80/udp").unwrap(),
        ];
        assert!(conflicts_with(&mappings[0], &mappings).is_empty());
        assert!(conflicts_with(&mappings[2], &mappings).is_empty());
    }

    #[test]
    fn dynamic_ports_never_conflict() {
        let mappings = vec![
            mapping("a.container", "80").unwrap(),
            mapping("b.container", "80").unwrap(),
        ];
        assert!(conflicts_with(&mappings[0], &mappings).is_empty());
    }

    #[test]
    fn parses_exposed_port_forms() {
        let e = parse_exposed("80").unwrap();
        assert_eq!(
            (e.range.start, e.range.end, e.protocol),
            (80, 80, Protocol::Tcp)
        );
        let e = parse_exposed("8000-8010/udp").unwrap();
        assert_eq!(
            (e.range.start, e.range.end, e.protocol),
            (8000, 8010, Protocol::Udp)
        );
        assert!(parse_exposed("80/sctp").is_none());
        assert!(parse_exposed("http").is_none());
    }

    #[test]
    fn serves_matches_container_port_and_protocol() {
        let m = mapping("web.pod", "8081:80").unwrap();
        assert!(serves(&[parse_exposed("80/tcp").unwrap()], &m));
        assert!(serves(&[parse_exposed("79-81").unwrap()], &m));
        assert!(!serves(&[parse_exposed("80/udp").unwrap()], &m));
        assert!(!serves(&[parse_exposed("443").unwrap()], &m));
        assert!(!serves(&[], &m));
    }

    #[test]
    fn declared_exposed_reads_container_section_only() {
        let unit = QuadletUnit {
            file_name: "a.container".into(),
            group: String::new(),
            path: "/tmp/a.container".into(),
            kind: UnitKind::Container,
            sections: vec![crate::quadlet::model::Section {
                name: "Container".into(),
                entries: [
                    ("Image", "x"),
                    ("ExposePort", "80"),
                    ("ExposePort", "53/udp"),
                    ("ExposePort", "bad"),
                ]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            }],
            raw: String::new(),
        };
        assert_eq!(declared_exposed(&unit).len(), 2);
    }
}
