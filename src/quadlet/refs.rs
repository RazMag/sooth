//! Cross-unit reference resolution: which Container/Pod quadlets consume a
//! given Volume or Network unit. Pure and unit-testable -- `&[QuadletUnit]`
//! in, referencing file names out; no D-Bus, no filesystem.

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
}
