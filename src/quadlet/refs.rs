//! Cross-unit reference resolution: which Container/Pod quadlets consume a
//! given Volume or Network unit (`consumers_of`), and the reverse for a
//! Pod specifically -- which containers are its members and which
//! Volumes/Networks its own `[Pod]` section declares (`pod_members`,
//! `pod_own_refs`), for the pod detail page's "part of this pod" rows.
//! Pure and unit-testable -- `&[QuadletUnit]` in, referencing file names
//! out; no D-Bus, no filesystem. Also which podman secrets a Container
//! consumes via `Secret=` (`secret_refs` / `secret_consumers` /
//! `missing_secrets`) -- the existence check itself lives in `crate::secrets`
//! -- and which host `${NAME}` variables a unit interpolates (`env_refs` /
//! `missing_env`), checked against the user manager's live environment.

use std::collections::{BTreeMap, HashSet};

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

/// The image a Container unit runs, as podman would name it: its `Image=`
/// value, or -- when that names an `.image` / `.build` quadlet in `all` --
/// the image that quadlet produces (`ImageTag=`, else an `.image`'s own
/// `Image=`). `None` when there's no `Image=` or it names a quadlet that
/// isn't there / doesn't say what it produces.
pub fn container_image(container: &QuadletUnit, all: &[QuadletUnit]) -> Option<String> {
    let value = container.section("Container")?.get("Image")?.trim();
    let Some(kind) = [UnitKind::Image, UnitKind::Build]
        .into_iter()
        .find(|k| value.ends_with(&format!(".{}", k.extension())))
    else {
        return Some(value.to_string());
    };
    let target = all
        .iter()
        .find(|u| u.kind == kind && u.file_name == value)?;
    let section = target.section(kind.primary_section())?;
    section
        .get("ImageTag")
        .or_else(|| {
            (kind == UnitKind::Image)
                .then(|| section.get("Image"))
                .flatten()
        })
        .map(|s| s.trim().to_string())
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

/// The podman secret names a unit consumes: every `Secret=` in a Container's
/// `[Container]` section, trimmed to the name (the first `,`-delimited
/// segment of `NAME[,type=…,target=…]`). Sorted and deduplicated. Empty for
/// every other kind -- a `.build`'s `Secret=` is `podman build --secret
/// id=…,src=<file>`, which reads a host file, not the podman secret store.
pub fn secret_refs(unit: &QuadletUnit) -> Vec<String> {
    if unit.kind != UnitKind::Container {
        return Vec::new();
    }
    let Some(section) = unit.section("Container") else {
        return Vec::new();
    };
    let mut out: Vec<String> = section
        .entries
        .iter()
        .filter(|(k, _)| k.as_str() == "Secret")
        .filter_map(|(_, v)| {
            let name = v.split(',').next().unwrap_or(v).trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// File names of every unit in `all` that references the podman secret
/// `name` via `Secret=`. Sorted -- the secret analogue of [`consumers_of`].
pub fn secret_consumers(name: &str, all: &[QuadletUnit]) -> Vec<String> {
    let mut out: Vec<String> = all
        .iter()
        .filter(|u| secret_refs(u).iter().any(|s| s == name))
        .map(|u| u.file_name.clone())
        .collect();
    out.sort();
    out
}

/// Every secret name referenced by some unit in `units` that isn't in
/// `existing`, mapped to the (sorted) file names referencing it.
pub fn missing_secrets<'a>(
    units: impl IntoIterator<Item = &'a QuadletUnit>,
    existing: &HashSet<String>,
) -> BTreeMap<String, Vec<String>> {
    missing_by(units, existing, secret_refs)
}

/// The host variables a unit interpolates as `${NAME}` (any section, any
/// kind) -- the ones the systemd user manager has to supply, so minus any
/// the unit defines itself in `[Service] Environment=`. `$$` is systemd's
/// escaped literal `$` and never starts a reference. Sorted and deduplicated.
pub fn env_refs(unit: &QuadletUnit) -> Vec<String> {
    let local = local_env_names(unit);
    let mut out: Vec<String> = unit
        .sections
        .iter()
        .flat_map(|s| s.entries.iter())
        .flat_map(|(_, v)| interpolated_names(v))
        .filter(|n| !local.contains(n))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every `${NAME}` referenced by some unit in `units` that isn't in
/// `existing` (the manager's live environment), mapped to the (sorted) file
/// names referencing it -- the env analogue of [`missing_secrets`].
pub fn missing_env<'a>(
    units: impl IntoIterator<Item = &'a QuadletUnit>,
    existing: &HashSet<String>,
) -> BTreeMap<String, Vec<String>> {
    missing_by(units, existing, env_refs)
}

/// What one unit references that doesn't exist -- the list-table badge.
/// Each half is empty when its store couldn't be read, rather than flagging
/// every reference.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MissingRefs {
    pub secrets: Vec<String>,
    pub env: Vec<String>,
}

impl MissingRefs {
    /// `secrets` / `env`: the names that exist, or `None` when unknown.
    pub fn of(
        unit: &QuadletUnit,
        secrets: Option<&HashSet<String>>,
        env: Option<&HashSet<String>>,
    ) -> Self {
        let absent = |refs: Vec<String>, existing: Option<&HashSet<String>>| match existing {
            Some(e) => refs.into_iter().filter(|n| !e.contains(n)).collect(),
            None => Vec::new(),
        };
        Self {
            secrets: absent(secret_refs(unit), secrets),
            env: absent(env_refs(unit), env),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty() && self.env.is_empty()
    }

    pub fn len(&self) -> usize {
        self.secrets.len() + self.env.len()
    }
}

fn missing_by<'a>(
    units: impl IntoIterator<Item = &'a QuadletUnit>,
    existing: &HashSet<String>,
    refs: fn(&QuadletUnit) -> Vec<String>,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for unit in units {
        for name in refs(unit) {
            if !existing.contains(&name) {
                out.entry(name).or_default().push(unit.file_name.clone());
            }
        }
    }
    for users in out.values_mut() {
        users.sort();
    }
    out
}

/// Every well-formed `${NAME}` in `value`, in order. `$$` is skipped as an
/// escape; an unterminated or non-identifier `${…}` is ignored.
fn interpolated_names(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(i) = rest.find('$') {
        let after = &rest[i + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            rest = tail;
        } else if let Some(body) = after.strip_prefix('{') {
            match body.find('}') {
                Some(end) => {
                    let name = &body[..end];
                    if crate::hostenv::valid_name(name) {
                        out.push(name.to_string());
                    }
                    rest = &body[end + 1..];
                }
                None => break,
            }
        } else {
            rest = after;
        }
    }
    out
}

/// Names the unit's own `[Service] Environment=` lines define (systemd uses
/// them when expanding `${NAME}` too). Each line is space-separated
/// `KEY=VALUE` assignments, optionally quoted.
fn local_env_names(unit: &QuadletUnit) -> HashSet<String> {
    unit.section("Service")
        .into_iter()
        .flat_map(|s| s.entries.iter())
        .filter(|(k, _)| k.as_str() == "Environment")
        .flat_map(|(_, v)| v.split_whitespace())
        .filter_map(|tok| {
            let (name, _) = tok.trim_start_matches(['"', '\'']).split_once('=')?;
            crate::hostenv::valid_name(name).then(|| name.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quadlet::model::Section;

    #[test]
    fn secret_refs_take_the_name_segment() {
        let web = unit(
            "web.container",
            UnitKind::Container,
            "Container",
            &[
                ("Secret", "db-pass,type=env,target=DB_PASSWORD"),
                ("Secret", "tls-key"),
                ("Secret", " db-pass ,type=mount"),
                ("Secret", ""),
                ("Environment", "A=b"),
            ],
        );
        assert_eq!(secret_refs(&web), vec!["db-pass", "tls-key"]);
    }

    #[test]
    fn secret_refs_ignore_build_units() {
        let b = unit(
            "app.build",
            UnitKind::Build,
            "Build",
            &[("Secret", "id=token,src=/run/token")],
        );
        assert!(secret_refs(&b).is_empty());
    }

    #[test]
    fn secret_consumers_and_missing() {
        let all = vec![
            unit(
                "b.container",
                UnitKind::Container,
                "Container",
                &[("Secret", "shared"), ("Secret", "only-b")],
            ),
            unit(
                "a.container",
                UnitKind::Container,
                "Container",
                &[("Secret", "shared,type=env,target=S")],
            ),
        ];
        assert_eq!(
            secret_consumers("shared", &all),
            vec!["a.container", "b.container"]
        );
        let existing: HashSet<String> = ["only-b".to_string()].into();
        let missing = missing_secrets(&all, &existing);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing["shared"], vec!["a.container", "b.container"]);
    }

    #[test]
    fn env_refs_scan_every_section_and_skip_escapes_and_local_defs() {
        let mut u = unit(
            "web.container",
            UnitKind::Container,
            "Container",
            &[
                ("Image", "ghcr.io/x/${IMAGE_TAG}"),
                ("Volume", "${DATA_DIR}/a:/a"),
                ("Exec", "echo $${NOT_A_REF} ${bad-name} ${UNTERMINATED"),
                ("Label", "x=${DATA_DIR}"),
            ],
        );
        u.sections.push(Section {
            name: "Service".into(),
            entries: vec![
                ("Environment".into(), "\"IMAGE_TAG=latest\" OTHER=1".into()),
                ("ExecStartPre".into(), "/bin/true ${PRE_VAR}".into()),
            ],
        });
        assert_eq!(env_refs(&u), vec!["DATA_DIR", "PRE_VAR"]);
    }

    #[test]
    fn missing_refs_flag_only_known_stores() {
        let u = unit(
            "web.container",
            UnitKind::Container,
            "Container",
            &[
                ("Secret", "db"),
                ("Image", "${TAG}"),
                ("Volume", "${HOME}:/h"),
            ],
        );
        let secrets: HashSet<String> = HashSet::new();
        let env: HashSet<String> = ["HOME".to_string()].into();
        let m = MissingRefs::of(&u, Some(&secrets), Some(&env));
        assert_eq!(m.secrets, vec!["db"]);
        assert_eq!(m.env, vec!["TAG"]);
        assert_eq!(m.len(), 2);
        assert!(MissingRefs::of(&u, None, None).is_empty());
        assert_eq!(missing_env([&u], &env)["TAG"], vec!["web.container"]);
    }

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

    #[test]
    fn container_image_resolves_image_and_build_quadlets() {
        let c = |image: &str| {
            unit(
                "web.container",
                UnitKind::Container,
                "Container",
                &[("Image", image)],
            )
        };
        let all = vec![
            unit(
                "nginx.image",
                UnitKind::Image,
                "Image",
                &[("Image", "docker.io/library/nginx")],
            ),
            unit(
                "app.build",
                UnitKind::Build,
                "Build",
                &[("ImageTag", "localhost/app")],
            ),
        ];
        assert_eq!(
            container_image(&c("docker.io/x"), &all).as_deref(),
            Some("docker.io/x")
        );
        assert_eq!(
            container_image(&c("nginx.image"), &all).as_deref(),
            Some("docker.io/library/nginx")
        );
        assert_eq!(
            container_image(&c("app.build"), &all).as_deref(),
            Some("localhost/app")
        );
        assert_eq!(container_image(&c("gone.image"), &all), None);
    }
}
