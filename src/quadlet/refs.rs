//! Cross-unit reference resolution: which Container/Pod quadlets consume a
//! given Volume or Network unit (`consumers_of`), and the reverse for a
//! Pod specifically -- which containers are its members and which
//! Volumes/Networks its own `[Pod]` section declares (`pod_members`,
//! `pod_own_refs`), for the pod detail page's "part of this pod" rows.
//! Pure and unit-testable -- `&[QuadletUnit]` in, referencing file names
//! out; no D-Bus, no filesystem.

use super::model::{QuadletUnit, UnitKind};
use super::naming;

/// The podman-visible resource name a Volume/Network quadlet produces: the
/// explicit `VolumeName=` / `NetworkName=` if set, else podman's default of
/// `systemd-<stem>`.
fn resource_name(target: &QuadletUnit) -> Option<String> {
    let (section, key) = match target.kind {
        UnitKind::Volume => ("Volume", "VolumeName"),
        UnitKind::Network => ("Network", "NetworkName"),
        _ => return None,
    };
    Some(
        target
            .section(section)
            .and_then(|s| s.get(key))
            .map(str::to_string)
            .unwrap_or_else(|| format!("systemd-{}", naming::stem(&target.file_name))),
    )
}

/// File names of every Container/Pod unit in `all` that references `target`
/// (a Volume or Network quadlet) -- either by its quadlet file name
/// (`Volume=data.volume:/x`, the form podman-systemd.unit(5) recommends) or
/// by the podman resource name it generates (`Volume=systemd-data:/x`).
/// Sorted and deduplicated. Empty for any other kind.
pub fn consumers_of(target: &QuadletUnit, all: &[QuadletUnit]) -> Vec<String> {
    let key = match target.kind {
        UnitKind::Volume => "Volume",
        UnitKind::Network => "Network",
        _ => return Vec::new(),
    };
    let Some(resource) = resource_name(target) else {
        return Vec::new();
    };

    let mut out: Vec<String> = all
        .iter()
        .filter(|u| matches!(u.kind, UnitKind::Container | UnitKind::Pod))
        .filter(|u| {
            let Some(section) = u.section(u.kind.primary_section()) else {
                return false;
            };
            section
                .entries
                .iter()
                .filter(|(k, _)| k.as_str() == key)
                .any(|(_, v)| {
                    // `Volume=SOURCE:DEST[:OPTS]` / `Network=NAME[:OPTS]` --
                    // only the first `:`-delimited segment names the resource.
                    let source = v.split(':').next().unwrap_or(v).trim();
                    !source.is_empty() && (source == target.file_name || source == resource)
                })
        })
        .map(|u| u.file_name.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// File names of every Container quadlet in `all` whose `Pod=` points at
/// `pod` -- its actual member containers. Quadlet's pod-membership model is
/// inverted (a `.pod` file never lists them; each container opts itself in),
/// so this is the one place that reverses it back into a list. Sorted.
pub fn pod_members(pod: &QuadletUnit, all: &[QuadletUnit]) -> Vec<String> {
    let mut out: Vec<String> = all
        .iter()
        .filter(|u| u.kind == UnitKind::Container)
        .filter(|u| {
            u.section("Container").and_then(|s| s.get("Pod")) == Some(pod.file_name.as_str())
        })
        .map(|u| u.file_name.clone())
        .collect();
    out.sort();
    out
}

/// The networks and volumes (in that order) a Pod's own `[Pod]` section
/// declares via `Network=`/`Volume=`, trimmed to just the resource
/// reference (the part before any `:OPTS`/`:DEST`) and resolved against
/// `all` so a value written as podman's generated resource name
/// (`systemd-data`) still links back to its quadlet file, the same as
/// `consumers_of` resolves in the other direction. Anything that doesn't
/// match a known quadlet -- `host`, a bind-mount path, an externally
/// created resource -- passes through unresolved; `unit_links` already
/// renders an unresolved name as plain text instead of a link.
pub fn pod_own_refs(pod: &QuadletUnit, all: &[QuadletUnit]) -> (Vec<String>, Vec<String>) {
    let Some(section) = pod.section("Pod") else {
        return (Vec::new(), Vec::new());
    };
    let resolve = |key: &str, kind: UnitKind| -> Vec<String> {
        let mut out: Vec<String> = section
            .entries
            .iter()
            .filter(|(k, _)| k.as_str() == key)
            .filter_map(|(_, v)| {
                let source = v.split(':').next().unwrap_or(v).trim();
                if source.is_empty() {
                    return None;
                }
                Some(
                    all.iter()
                        .filter(|u| u.kind == kind)
                        .find(|u| {
                            u.file_name == source || resource_name(u).as_deref() == Some(source)
                        })
                        .map_or_else(|| source.to_string(), |u| u.file_name.clone()),
                )
            })
            .collect();
        out.sort();
        out.dedup();
        out
    };
    (
        resolve("Network", UnitKind::Network),
        resolve("Volume", UnitKind::Volume),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quadlet::model::Section;

    fn unit(
        file_name: &str,
        kind: UnitKind,
        section: &str,
        entries: &[(&str, &str)],
    ) -> QuadletUnit {
        QuadletUnit {
            file_name: file_name.into(),
            group: String::new(),
            path: format!("/tmp/{file_name}").into(),
            kind,
            sections: vec![Section {
                name: section.into(),
                entries: entries
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }],
            raw: String::new(),
        }
    }

    #[test]
    fn matches_quadlet_file_name_and_generated_name() {
        let vol = unit("data.volume", UnitKind::Volume, "Volume", &[]);
        let all = vec![
            vol.clone(),
            unit(
                "web.container",
                UnitKind::Container,
                "Container",
                &[("Volume", "data.volume:/var/www")],
            ),
            unit(
                "db.container",
                UnitKind::Container,
                "Container",
                &[("Volume", "systemd-data:/data")],
            ),
            unit(
                "cache.container",
                UnitKind::Container,
                "Container",
                &[("Volume", "/host/path:/data")],
            ),
        ];
        assert_eq!(
            consumers_of(&vol, &all),
            vec!["db.container", "web.container"]
        );
    }

    #[test]
    fn respects_explicit_volume_name() {
        let vol = unit(
            "data.volume",
            UnitKind::Volume,
            "Volume",
            &[("VolumeName", "shared")],
        );
        let all = vec![
            vol.clone(),
            unit(
                "a.container",
                UnitKind::Container,
                "Container",
                &[("Volume", "shared:/data")],
            ),
            unit(
                "b.container",
                UnitKind::Container,
                "Container",
                &[("Volume", "systemd-data:/data")],
            ),
        ];
        assert_eq!(consumers_of(&vol, &all), vec!["a.container"]);
    }

    #[test]
    fn network_consumers_include_pods_and_extra_options() {
        let net = unit("frontend.network", UnitKind::Network, "Network", &[]);
        let all = vec![
            net.clone(),
            unit(
                "web.container",
                UnitKind::Container,
                "Container",
                &[("Network", "frontend.network")],
            ),
            unit(
                "api.pod",
                UnitKind::Pod,
                "Pod",
                &[("Network", "frontend.network:alias=api")],
            ),
            unit(
                "other.container",
                UnitKind::Container,
                "Container",
                &[("Network", "host")],
            ),
        ];
        assert_eq!(consumers_of(&net, &all), vec!["api.pod", "web.container"]);
    }

    #[test]
    fn ignores_non_resource_kinds() {
        let img = unit("x.image", UnitKind::Image, "Image", &[]);
        assert!(consumers_of(&img, std::slice::from_ref(&img)).is_empty());
    }

    #[test]
    fn pod_members_finds_containers_pointing_back_and_sorts_them() {
        let pod = unit("web.pod", UnitKind::Pod, "Pod", &[]);
        let all = vec![
            pod.clone(),
            unit(
                "b.container",
                UnitKind::Container,
                "Container",
                &[("Pod", "web.pod")],
            ),
            unit(
                "a.container",
                UnitKind::Container,
                "Container",
                &[("Pod", "web.pod")],
            ),
            unit(
                "other.container",
                UnitKind::Container,
                "Container",
                &[("Pod", "other.pod")],
            ),
            unit(
                "standalone.container",
                UnitKind::Container,
                "Container",
                &[],
            ),
        ];
        assert_eq!(pod_members(&pod, &all), vec!["a.container", "b.container"]);
    }

    #[test]
    fn pod_own_refs_reads_the_pods_own_section_and_resolves_by_file_name_or_resource_name() {
        let pod = unit(
            "web.pod",
            UnitKind::Pod,
            "Pod",
            &[
                ("Network", "frontend.network:alias=api"),
                ("Volume", "systemd-data:/data"),
                ("Volume", "/host/bind:/x"),
            ],
        );
        let all = vec![
            pod.clone(),
            unit("frontend.network", UnitKind::Network, "Network", &[]),
            unit(
                "data.volume",
                UnitKind::Volume,
                "Volume",
                &[("VolumeName", "shared")],
            ),
        ];
        // `systemd-data` isn't `data.volume`'s resource name (it declares an
        // explicit VolumeName=shared), so it stays unresolved -- as does the
        // bind-mount path, which never matches a quadlet file.
        let (networks, volumes) = pod_own_refs(&pod, &all);
        assert_eq!(networks, vec!["frontend.network"]);
        assert_eq!(volumes, vec!["/host/bind", "systemd-data"]);
    }

    #[test]
    fn pod_own_refs_resolves_the_generated_resource_name() {
        let pod = unit(
            "web.pod",
            UnitKind::Pod,
            "Pod",
            &[("Volume", "systemd-data:/data")],
        );
        let all = vec![
            pod.clone(),
            unit("data.volume", UnitKind::Volume, "Volume", &[]),
        ];
        let (_, volumes) = pod_own_refs(&pod, &all);
        assert_eq!(volumes, vec!["data.volume"]);
    }

    #[test]
    fn pod_own_refs_empty_when_no_pod_section() {
        let not_a_pod = unit("x.container", UnitKind::Container, "Container", &[]);
        assert_eq!(pod_own_refs(&not_a_pod, &[]), (Vec::new(), Vec::new()));
    }
}
