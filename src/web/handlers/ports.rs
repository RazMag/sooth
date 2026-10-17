use axum::Form;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, FragmentError, PageError};
use crate::imageinfo;
use crate::quadlet::ports;
use crate::quadlet::{QuadletError, QuadletUnit, UnitKind, discovery, refs};
use crate::web::core;
use crate::web::templates::ports::{HostNet, PodTargets, PortRow, UnitRef};
use crate::web::templates::{self};

/// Builds every declared `PublishPort=` mapping across Containers and Pods
/// (and each `Network=host` container's recorded ports) into `PortRow`s
/// (owner, the container(s) actually behind the port, live statuses,
/// conflicting units), plus the host-network containers themselves as
/// `HostNet`s, and hands both slices to `render`. They borrow from the
/// loaded units and the sorted mappings, so both have to stay on this stack
/// frame -- the closure keeps the borrow scoped without leaking those types
/// into a return signature.
async fn with_rows<T>(
    state: &AppState,
    render: impl FnOnce(&[PortRow], &[HostNet]) -> T,
) -> Result<T, AppError> {
    // `all` (every kind) so a member's `Image=foo.image` / `foo.build`
    // resolves to the image it names.
    let (units, all) =
        core::load_units_and_siblings(state, &[UnitKind::Container, UnitKind::Pod]).await?;
    let unit_refs: Vec<_> = units.iter().map(|(u, _)| u.clone()).collect();

    let mut mappings = ports::extract(&unit_refs);
    mappings.sort_by_key(|m| m.host_port.map(|r| r.start).unwrap_or(u16::MAX));

    // Which port(s) each pod member says it serves: its own `ExposeHostPort=`
    // plus its image's `EXPOSE`. Only pods with published ports need this.
    let member_files: Vec<String> = units
        .iter()
        .filter(|(u, _)| {
            u.kind == UnitKind::Pod && mappings.iter().any(|m| m.file_name == u.file_name)
        })
        .flat_map(|(pod, _)| refs::pod_members(pod, &unit_refs))
        .collect();
    let member_units: Vec<&QuadletUnit> = unit_refs
        .iter()
        .filter(|u| member_files.contains(&u.file_name))
        .collect();
    let core::MemberExposure {
        ports: member_ports,
        unpulled,
    } = core::member_exposure(&member_units, &all).await;

    let unit_ref = |file_name: &str| {
        units
            .iter()
            .find(|(u, _)| u.file_name == file_name)
            .map(|(u, s)| UnitRef {
                file_name: u.file_name.clone(),
                href: core::unit_url(u),
                status: s,
                unpulled: None,
            })
    };

    let rows: Vec<PortRow> = mappings
        .iter()
        .filter_map(|m| {
            let (unit, status) = units.iter().find(|(u, _)| u.file_name == m.file_name)?;
            let owner = UnitRef {
                file_name: unit.file_name.clone(),
                href: core::unit_url(unit),
                status,
                unpulled: None,
            };
            // A pod's ports land on whichever of its containers (`Pod=` on
            // the container side) listens on the container port: the
            // member(s) it's assigned to, else the ones declaring that port.
            // If none does, it stays on the pod.
            let members = if unit.kind == UnitKind::Pod {
                let all_members: Vec<(String, ports::MemberPorts)> =
                    refs::pod_members(unit, &unit_refs)
                        .into_iter()
                        .map(|f| {
                            let p = member_ports.get(&f).cloned().unwrap_or_default();
                            (f, p)
                        })
                        .collect();
                Some(PodTargets {
                    serving: ports::serving(m, &all_members)
                        .into_iter()
                        .filter_map(|(f, why)| Some((unit_ref(f)?, why)))
                        .collect(),
                    unpulled: all_members
                        .iter()
                        .filter_map(|(f, _)| {
                            let mut r = unit_ref(f)?;
                            r.unpulled = Some(unpulled.get(f)?.clone());
                            Some(r)
                        })
                        .collect(),
                    members: all_members.into_iter().map(|(f, _)| f).collect(),
                })
            } else {
                None
            };
            let conflicts = ports::conflicts_with(m, &mappings)
                .iter()
                .map(|f| match unit_ref(f) {
                    Some(r) => (r.file_name, Some(r.href)),
                    None => (f.clone(), None),
                })
                .collect();
            Some(PortRow {
                mapping: m,
                owner,
                members,
                conflicts,
            })
        })
        .collect();

    let hosts: Vec<HostNet> = units
        .iter()
        .filter(|(u, _)| ports::is_host_network(u))
        .filter_map(|(u, _)| {
            Some(HostNet {
                unit: unit_ref(&u.file_name)?,
                ports: ports::host_ports(u),
            })
        })
        .collect();

    Ok(render(&rows, &hosts))
}

async fn csrf(session: &Session) -> String {
    crate::auth::csrf::current(session)
        .await
        .unwrap_or_default()
}

pub async fn index(
    State(state): State<AppState>,
    session: Session,
) -> Result<impl IntoResponse, PageError> {
    let health = state.health.get();
    let csrf = csrf(&session).await;
    Ok(with_rows(&state, |rows, hosts| {
        templates::ports::ports_page(rows, hosts, &csrf, health)
    })
    .await?)
}

/// The `<tbody>` rows only -- re-fetched by the Ports table on
/// `sse:any-status` / `sse:units-changed` so a unit starting or stopping
/// flips its Host Port cell between a greyed pill and a live link.
pub async fn rows(
    State(state): State<AppState>,
    session: Session,
) -> Result<maud::Markup, FragmentError> {
    let csrf = csrf(&session).await;
    Ok(with_rows(&state, |rows, _| {
        templates::ports::ports_rows(rows, &csrf, None)
    })
    .await?)
}

#[derive(Deserialize)]
pub struct PullForm {
    csrf_token: String,
    /// The pod member whose image to pull -- the image itself is resolved
    /// from its quadlet, never taken from the request.
    file_name: String,
}

/// Pulls a pod member's image so its `EXPOSE` can be read, then answers with
/// the re-rendered `<tbody>` (a failed pull shows podman's message under
/// that member rather than an error page -- htmx drops non-2xx bodies).
pub async fn pull(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<PullForm>,
) -> Result<maud::Markup, FragmentError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let all = discovery::load_all(&state.quadlet_dir)?;
    let image = core::pullable_image(&form.file_name, &all)
        .ok_or_else(|| AppError::NotFound(format!("no pullable image for {}", form.file_name)))?;
    let error = imageinfo::pull(&image).await.err();
    let csrf = csrf(&session).await;
    let pull_error = error.as_deref().map(|e| (form.file_name.as_str(), e));
    Ok(with_rows(&state, |rows, _| {
        templates::ports::ports_rows(rows, &csrf, pull_error)
    })
    .await?)
}

#[derive(Deserialize)]
pub struct AssignForm {
    csrf_token: String,
    /// The pod publishing the port.
    pod: String,
    /// Its container port, as `ExposedPort::spec` (`80/tcp`).
    port: String,
    /// The member to assign it to; empty = back to automatic matching.
    #[serde(default)]
    member: String,
}

/// Assigns a pod-published port to one of the pod's containers (or clears
/// the assignment) -- sooth bookkeeping only, see `core::assign_pod_port` --
/// then answers with the re-rendered `<tbody>`.
pub async fn assign(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<AssignForm>,
) -> Result<maud::Markup, FragmentError> {
    let member = Some(form.member.trim()).filter(|m| !m.is_empty());
    core::assign_pod_port(
        &state,
        &session,
        &form.csrf_token,
        &form.pod,
        &form.port,
        member,
    )
    .await?;
    let csrf = csrf(&session).await;
    Ok(with_rows(&state, |rows, _| {
        templates::ports::ports_rows(rows, &csrf, None)
    })
    .await?)
}

#[derive(Deserialize)]
pub struct HostPortForm {
    csrf_token: String,
    /// The `Network=host` container.
    file_name: String,
    /// `8080`, `8080/udp` or `8000-8010` -- anything `ports::parse_exposed`
    /// takes.
    port: String,
    /// Present = forget the port instead of recording it.
    #[serde(default)]
    remove: Option<String>,
}

/// Records (or removes) a port a `Network=host` container listens on --
/// sooth bookkeeping only, see `core::set_host_port` -- then answers with
/// the re-rendered page content, so a first port brings the ports table in
/// with it. A rejected port (malformed, not a host-network container) is
/// shown above the form instead, as a 200 since htmx drops non-2xx bodies.
pub async fn host_port(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<HostPortForm>,
) -> Result<maud::Markup, FragmentError> {
    let result = core::set_host_port(
        &state,
        &session,
        &form.csrf_token,
        &form.file_name,
        &form.port,
        form.remove.is_none(),
    )
    .await;
    let error = match result {
        Ok(()) => None,
        Err(AppError::Quadlet(QuadletError::Validation(msg))) => Some(msg),
        Err(e) => return Err(e.into()),
    };
    let csrf = csrf(&session).await;
    Ok(with_rows(&state, |rows, hosts| {
        templates::ports::ports_content(rows, hosts, &csrf, error.as_deref())
    })
    .await?)
}
