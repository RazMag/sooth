//! Parses `PublishPort=` declarations out of Container/Pod quadlet units and
//! detects host-port collisions between them. Pure and unit-testable: no
//! D-Bus, no filesystem, just `&[QuadletUnit]` in, structured data out.
//!
//! Also owns which pod member a pod-published port is shown against, and
//! why ([`Why`]): a manual assignment ([`assigned_ports`]) overrides the
//! member's declared ports ([`declared_exposed`], then its image's
//! `EXPOSE`). And the ports a `Network=host` container listens on, which
//! have no `PublishPort=` to read: recorded by hand ([`host_ports`]) and
//! listed by [`extract`] like any published port. Both hand-kept lists come
//! from the unit's `port_notes` (see `super::portnotes`), never its file.

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
    /// The original `PublishPort=` value, kept for display (for a
    /// host-network port, a description of it).
    pub raw: String,
    /// A port a `Network=host` container listens on straight on the host
    /// ([`host_ports`]), not a `PublishPort=`: `host_port` and
    /// `container_port` are the same port.
    pub host_network: bool,
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
/// Pod unit in `units`, plus each host-network container's recorded
/// [`host_ports`] (`host_network: true`). Unparsable entries are skipped,
/// never fatal -- a typo'd port line shouldn't take down the whole ports
/// screen.
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
        out.extend(host_ports(unit).into_iter().map(|p| PortMapping {
            file_name: unit.file_name.clone(),
            host_port: Some(p.range),
            protocol: p.protocol,
            container_port: p.range_str(),
            raw: format!("host network: {}", p.spec()),
            host_network: true,
        }));
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
        host_network: false,
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

/// A port a container says it serves -- an `ExposeHostPort=` line in its
/// quadlet (podman's `--expose`) or an `EXPOSE` baked into its image.
/// Informational only (podman publishes nothing for it), but it's the one
/// declared signal of which container in a pod is meant to receive a
/// pod-published port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExposedPort {
    pub range: HostPortRange,
    pub protocol: Protocol,
}

/// Parses `80`, `80/tcp` or `8000-8010/udp` -- the shape of both
/// `ExposeHostPort=` values and an image's `Config.ExposedPorts` keys.
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

/// Every `ExposeHostPort=` in a Container unit's `[Container]` section.
/// Unparsable values are skipped. Empty for every other kind.
pub fn declared_exposed(unit: &QuadletUnit) -> Vec<ExposedPort> {
    if unit.kind != UnitKind::Container {
        return Vec::new();
    }
    unit.section("Container")
        .map(|s| {
            s.entries
                .iter()
                .filter(|(k, _)| k == "ExposeHostPort")
                .filter_map(|(_, v)| parse_exposed(v))
                .collect()
        })
        .unwrap_or_default()
}

impl ExposedPort {
    /// The canonical `80/tcp` / `8000-8010/udp` form -- what
    /// `portnotes` stores, and what the Ports screen's assign picker posts
    /// back.
    pub fn spec(self) -> String {
        format!("{}/{}", self.range_str(), self.protocol.as_str())
    }

    /// `80` / `8000-8010`, without the protocol.
    pub fn range_str(self) -> String {
        let HostPortRange { start, end } = self.range;
        if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        }
    }

    /// The container port `mapping` forwards to, as an `ExposedPort` --
    /// `None` only for a malformed one, which [`extract`] never yields.
    pub fn of_mapping(mapping: &PortMapping) -> Option<Self> {
        Some(Self {
            range: parse_port_range(&mapping.container_port)?,
            protocol: mapping.protocol,
        })
    }
}

/// The pod ports assigned to a Container unit by hand (its `port_notes`).
/// Empty for every other kind.
pub fn assigned_ports(unit: &QuadletUnit) -> Vec<ExposedPort> {
    if unit.kind != UnitKind::Container {
        return Vec::new();
    }
    unit.port_notes.serves.clone()
}

/// Whether a Container unit runs on the host's network (`Network=host`).
pub fn is_host_network(unit: &QuadletUnit) -> bool {
    unit.kind == UnitKind::Container
        && unit.section("Container").is_some_and(|s| {
            s.entries
                .iter()
                .any(|(k, v)| k == "Network" && v.trim() == "host")
        })
}

/// The ports recorded by hand (its `port_notes`) for a host-network
/// Container unit. Empty for any other unit, even one with ports recorded
/// -- without `Network=host` its `PublishPort=` lines are what reach the
/// host.
pub fn host_ports(unit: &QuadletUnit) -> Vec<ExposedPort> {
    if !is_host_network(unit) {
        return Vec::new();
    }
    unit.port_notes.host.clone()
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

/// What a pod member says about the ports it serves.
#[derive(Debug, Clone, Default)]
pub struct MemberPorts {
    /// Pod ports assigned to it by hand ([`assigned_ports`]).
    pub assigned: Vec<ExposedPort>,
    /// Its own `ExposeHostPort=` lines ([`declared_exposed`]).
    pub declared: Vec<ExposedPort>,
    /// Its image and that image's `EXPOSE`, when the image could be
    /// inspected.
    pub image: Option<(String, Vec<ExposedPort>)>,
}

/// Why a pod-published port is shown against a member -- strongest first,
/// the order [`MemberPorts::why`] checks them in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// Assigned by hand ([`assigned_ports`]).
    Assigned,
    /// The member's own `ExposeHostPort=`.
    Declared,
    /// `EXPOSE` in the member's image (named).
    Image(String),
}

impl MemberPorts {
    /// Whether, and why, this member serves `mapping`'s container port.
    pub fn why(&self, mapping: &PortMapping) -> Option<Why> {
        if serves(&self.assigned, mapping) {
            Some(Why::Assigned)
        } else if serves(&self.declared, mapping) {
            Some(Why::Declared)
        } else {
            let (image, exposed) = self.image.as_ref()?;
            serves(exposed, mapping).then(|| Why::Image(image.clone()))
        }
    }
}

/// The members a pod-published `mapping` reaches, each with why: the ones
/// it's assigned to by hand if any (an assignment overrides every declared
/// match), else every member declaring its container port. Empty when
/// neither -- the port stays on the pod.
pub fn serving<'a>(
    mapping: &PortMapping,
    members: &'a [(String, MemberPorts)],
) -> Vec<(&'a String, Why)> {
    let all: Vec<(&String, Why)> = members
        .iter()
        .filter_map(|(file, p)| Some((file, p.why(mapping)?)))
        .collect();
    if all.iter().any(|(_, why)| *why == Why::Assigned) {
        all.into_iter()
            .filter(|(_, why)| *why == Why::Assigned)
            .collect()
    } else {
        all
    }
}

/// The ports a list row shows once a pod's `PublishPort=` mappings have been
/// split between the pod and its members (see [`split_pod_ports`]).
#[derive(Debug, Clone, Default)]
pub struct SplitPorts {
    /// The pod publishing these ports (for a member's tooltip).
    pub pod: String,
    /// For a member: the pod mappings it serves, each with why. For the pod:
    /// the ones no member serves (`None`).
    pub mappings: Vec<(PortMapping, Option<Why>)>,
    /// For the pod: how many of its mappings moved to its members. Always 0
    /// for a member.
    pub moved: usize,
}

/// Splits a pod's published ports between the pod and the `members` that
/// serve them, as the Ports screen matches them: a mapping goes to every
/// member [`serving`] picks; one it picks none for stays on the pod. Returns
/// the pod's entry (keyed `pod_file`) plus one per member that serves
/// anything.
pub fn split_pod_ports(
    pod_file: &str,
    pod_mappings: &[PortMapping],
    members: &[(String, MemberPorts)],
) -> Vec<(String, SplitPorts)> {
    let mut pod = SplitPorts {
        pod: pod_file.to_string(),
        ..SplitPorts::default()
    };
    let mut by_member: Vec<(String, SplitPorts)> = Vec::new();
    for m in pod_mappings {
        let serving = serving(m, members);
        if serving.is_empty() {
            pod.mappings.push((m.clone(), None));
            continue;
        }
        pod.moved += 1;
        for (file, why) in serving {
            let entry = (m.clone(), Some(why));
            match by_member.iter_mut().find(|(f, _)| f == file) {
                Some((_, split)) => split.mappings.push(entry),
                None => by_member.push((
                    file.clone(),
                    SplitPorts {
                        pod: pod_file.to_string(),
                        mappings: vec![entry],
                        moved: 0,
                    },
                )),
            }
        }
    }
    by_member.push((pod_file.to_string(), pod));
    by_member
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
    use crate::quadlet::portnotes::PortNotes;

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
    fn split_pod_ports_moves_served_ports_to_members() {
        let pod: Vec<PortMapping> = ["8080:80", "8443:443", "5353:53/udp", "9000:9000"]
            .iter()
            .map(|raw| mapping("web.pod", raw).unwrap())
            .collect();
        let exposed = |list: &[&str]| MemberPorts {
            declared: list.iter().map(|s| parse_exposed(s).unwrap()).collect(),
            ..MemberPorts::default()
        };
        let members = vec![
            ("web.container".to_string(), exposed(&["80", "443"])),
            ("dns.container".to_string(), exposed(&["53/udp"])),
            ("proxy.container".to_string(), exposed(&["80"])),
            ("idle.container".to_string(), exposed(&[])),
        ];
        let split = split_pod_ports("web.pod", &pod, &members);
        let raws = |file: &str| -> Vec<&str> {
            split
                .iter()
                .find(|(f, _)| f == file)
                .map(|(_, s)| s.mappings.iter().map(|(m, _)| m.raw.as_str()).collect())
                .unwrap_or_default()
        };
        assert_eq!(raws("web.container"), ["8080:80", "8443:443"]);
        // Served by two members: shown on both.
        assert_eq!(raws("proxy.container"), ["8080:80"]);
        assert_eq!(raws("dns.container"), ["5353:53/udp"]);
        // No member serves 9000, so it stays on the pod.
        assert_eq!(raws("web.pod"), ["9000:9000"]);
        let pod_entry = &split.iter().find(|(f, _)| f == "web.pod").unwrap().1;
        assert_eq!(pod_entry.moved, 3);
        // A member that serves nothing gets no entry.
        assert!(split.iter().all(|(f, _)| f != "idle.container"));
    }

    #[test]
    fn declared_exposed_reads_expose_host_port() {
        let unit = QuadletUnit {
            file_name: "a.container".into(),
            group: String::new(),
            path: "/tmp/a.container".into(),
            kind: UnitKind::Container,
            sections: vec![crate::quadlet::model::Section {
                name: "Container".into(),
                entries: [
                    ("Image", "x"),
                    ("ExposeHostPort", "80"),
                    ("ExposeHostPort", "5000-5002/udp"),
                    ("ExposeHostPort", "bad"),
                    // Not a quadlet key -- podman's generator rejects it.
                    ("ExposePort", "443"),
                ]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            }],
            raw: String::new(),
            port_notes: PortNotes::default(),
        };
        assert_eq!(
            declared_exposed(&unit),
            [
                parse_exposed("80").unwrap(),
                parse_exposed("5000-5002/udp").unwrap()
            ]
        );
    }

    fn container(raw: &str, port_notes: PortNotes) -> QuadletUnit {
        QuadletUnit {
            file_name: "a.container".into(),
            group: String::new(),
            path: "/tmp/a.container".into(),
            kind: UnitKind::Container,
            sections: crate::quadlet::parser::parse(raw).unwrap(),
            raw: raw.into(),
            port_notes,
        }
    }

    fn ports(list: &[&str]) -> Vec<ExposedPort> {
        list.iter().map(|s| parse_exposed(s).unwrap()).collect()
    }

    #[test]
    fn assigned_ports_come_from_port_notes_not_the_file() {
        let notes = PortNotes {
            serves: ports(&["80", "53/udp"]),
            ..PortNotes::default()
        };
        let unit = container("[Container]\nImage=x\n", notes);
        assert_eq!(assigned_ports(&unit), ports(&["80/tcp", "53/udp"]));
        // The `[Unit]` key 0.6.0 wrote is no longer read.
        let unit = container(
            "[Unit]\nX-Sooth-ServesPort=80\n\n[Container]\nImage=x\n",
            PortNotes::default(),
        );
        assert!(assigned_ports(&unit).is_empty());
    }

    #[test]
    fn host_network_ports_are_extracted_as_mappings() {
        let notes = PortNotes {
            serves: ports(&["80"]),
            host: ports(&["8080", "5000-5002/udp"]),
        };
        let unit = container("[Container]\nImage=x\nNetwork=host\n", notes);
        assert!(is_host_network(&unit));
        let mappings = extract(std::slice::from_ref(&unit));
        let got: Vec<_> = mappings
            .iter()
            .map(|m| {
                (
                    m.host_port.map(|r| (r.start, r.end)),
                    m.protocol,
                    m.container_port.as_str(),
                    m.host_network,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (Some((8080, 8080)), Protocol::Tcp, "8080", true),
                (Some((5000, 5002)), Protocol::Udp, "5000-5002", true),
            ]
        );
        // They collide with other units' published ports like any other.
        let other = mapping("b.container", "8080:80").unwrap();
        let all = [mappings[0].clone(), other];
        assert_eq!(conflicts_with(&all[0], &all), ["b.container"]);
    }

    #[test]
    fn host_ports_need_network_host() {
        let notes = PortNotes {
            host: ports(&["8080"]),
            ..PortNotes::default()
        };
        let unit = container("[Container]\nImage=x\nNetwork=bridge\n", notes);
        assert!(!is_host_network(&unit));
        assert!(host_ports(&unit).is_empty());
        assert!(extract(std::slice::from_ref(&unit)).is_empty());
    }

    #[test]
    fn exposed_port_spec_is_canonical() {
        assert_eq!(parse_exposed("80").unwrap().spec(), "80/tcp");
        assert_eq!(
            parse_exposed("8000-8010/udp").unwrap().spec(),
            "8000-8010/udp"
        );
        let m = mapping("web.pod", "8443:443").unwrap();
        assert_eq!(ExposedPort::of_mapping(&m).unwrap().spec(), "443/tcp");
    }

    #[test]
    fn assignment_overrides_declared_ports() {
        let port = |s: &str| parse_exposed(s).unwrap();
        let members = vec![
            (
                "app.container".to_string(),
                MemberPorts {
                    image: Some(("nginx".into(), vec![port("80")])),
                    ..MemberPorts::default()
                },
            ),
            (
                "proxy.container".to_string(),
                MemberPorts {
                    declared: vec![port("80")],
                    assigned: vec![port("80")],
                    ..MemberPorts::default()
                },
            ),
            (
                "side.container".to_string(),
                MemberPorts {
                    assigned: vec![port("9000")],
                    ..MemberPorts::default()
                },
            ),
        ];
        let names = |m: &str| -> Vec<(String, Why)> {
            serving(&mapping("p.pod", m).unwrap(), &members)
                .into_iter()
                .map(|(f, why)| (f.clone(), why))
                .collect()
        };
        assert_eq!(
            names("8080:80"),
            [("proxy.container".into(), Why::Assigned)]
        );
        // Assigned to a member that doesn't EXPOSE it at all.
        assert_eq!(
            names("9000:9000"),
            [("side.container".into(), Why::Assigned)]
        );
        assert!(names("5432:5432").is_empty());

        let split = split_pod_ports(
            "p.pod",
            &[
                mapping("p.pod", "8080:80").unwrap(),
                mapping("p.pod", "5432:5432").unwrap(),
            ],
            &members,
        );
        assert!(split.iter().all(|(f, _)| f != "app.container"));
        let pod = &split.iter().find(|(f, _)| f == "p.pod").unwrap().1;
        assert_eq!(pod.moved, 1);
        assert_eq!(pod.mappings[0].0.raw, "5432:5432");
        assert_eq!(pod.mappings[0].1, None);
    }

    #[test]
    fn why_prefers_declared_over_image() {
        let port = |s: &str| parse_exposed(s).unwrap();
        let both = MemberPorts {
            declared: vec![port("80")],
            image: Some(("nginx".into(), vec![port("80"), port("443")])),
            ..MemberPorts::default()
        };
        let m = |raw: &str| mapping("p.pod", raw).unwrap();
        assert_eq!(both.why(&m("8080:80")), Some(Why::Declared));
        assert_eq!(both.why(&m("8443:443")), Some(Why::Image("nginx".into())));
        assert_eq!(both.why(&m("9000:9000")), None);
        // Several members declaring a port all serve it.
        let members = vec![
            ("a.container".to_string(), both.clone()),
            ("b.container".to_string(), both),
        ];
        assert_eq!(serving(&m("8080:80"), &members).len(), 2);
    }
}
