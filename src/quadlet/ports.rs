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

/// A group of mappings from more than one quadlet file whose host-port
/// ranges overlap on the same protocol -- a real declared conflict.
#[derive(Debug, Clone)]
pub struct PortCollision {
    pub protocol: Protocol,
    pub host_port: HostPortRange,
    pub mappings: Vec<PortMapping>,
}

/// Host IP is deliberately ignored when grouping: over-flagging a
/// same-port-different-IP bind is safer than missing a real conflict on a
/// single-host rootless setup.
pub fn find_collisions(mappings: &[PortMapping]) -> Vec<PortCollision> {
    let mut groups: Vec<PortCollision> = Vec::new();

    for mapping in mappings {
        let Some(host_port) = mapping.host_port else {
            continue;
        };
        if let Some(group) = groups
            .iter_mut()
            .find(|g| g.protocol == mapping.protocol && g.host_port.overlaps(host_port))
        {
            group.host_port.start = group.host_port.start.min(host_port.start);
            group.host_port.end = group.host_port.end.max(host_port.end);
            group.mappings.push(mapping.clone());
        } else {
            groups.push(PortCollision {
                protocol: mapping.protocol,
                host_port,
                mappings: vec![mapping.clone()],
            });
        }
    }

    groups.retain(|g| {
        g.mappings
            .iter()
            .map(|m| &m.file_name)
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 1
    });
    groups
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
    fn detects_collision_across_two_files() {
        let mappings = vec![
            mapping("a.container", "8080:80").unwrap(),
            mapping("b.container", "8080:8081").unwrap(),
            mapping("c.container", "9090:90").unwrap(),
        ];
        let collisions = find_collisions(&mappings);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].mappings.len(), 2);
    }

    #[test]
    fn no_collision_for_same_file_or_different_ports() {
        let mappings = vec![
            mapping("a.container", "8080:80").unwrap(),
            mapping("a.container", "8080:81").unwrap(),
        ];
        assert!(find_collisions(&mappings).is_empty());
    }

    #[test]
    fn dynamic_ports_excluded_from_collisions() {
        let mappings = vec![
            mapping("a.container", "80").unwrap(),
            mapping("b.container", "80").unwrap(),
        ];
        assert!(find_collisions(&mappings).is_empty());
    }
}
