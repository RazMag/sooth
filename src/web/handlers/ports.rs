use std::collections::HashMap;

use axum::Form;
use axum::extract::State;
use axum::response::IntoResponse;
use futures_util::future::join_all;
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, FragmentError, PageError};
use crate::imageinfo;
use crate::quadlet::ports;
use crate::quadlet::{QuadletUnit, UnitKind, discovery, refs};
use crate::web::core;
use crate::web::templates::ports::{PodTargets, PortRow, UnitRef};
use crate::web::templates::{self};

/// Builds every declared `PublishPort=` mapping across Containers and Pods
/// into `PortRow`s (owner, the container(s) actually behind the port, live
/// statuses, conflicting units) and hands the slice to `render`. `PortRow`
/// borrows from the loaded units and the sorted mappings, so both have to
/// stay on this stack frame -- the closure keeps the borrow scoped without
/// leaking those types into a return signature.
async fn with_rows<T>(
    state: &AppState,
    render: impl FnOnce(&[PortRow]) -> T,
) -> Result<T, AppError> {
    // `all` (every kind) so a member's `Image=foo.image` / `foo.build`
    // resolves to the image it names.
    let (units, all) =
        core::load_units_and_siblings(state, &[UnitKind::Container, UnitKind::Pod]).await?;
    let unit_refs: Vec<_> = units.iter().map(|(u, _)| u.clone()).collect();

    let mut mappings = ports::extract(&unit_refs);
    mappings.sort_by_key(|m| m.host_port.map(|r| r.start).unwrap_or(u16::MAX));

    // Which port(s) each pod member says it serves: its own `ExposePort=`
    // plus its image's `EXPOSE`. Only pods with published ports need this,
    // and each distinct image is inspected once, concurrently.
    let member_files: Vec<String> = units
        .iter()
        .filter(|(u, _)| {
            u.kind == UnitKind::Pod && mappings.iter().any(|m| m.file_name == u.file_name)
        })
        .flat_map(|(pod, _)| refs::pod_members(pod, &unit_refs))
        .collect();
    let member_images: HashMap<&str, String> = member_files
        .iter()
        .filter_map(|f| {
            let u = unit_refs.iter().find(|u| &u.file_name == f)?;
            Some((u.file_name.as_str(), refs::container_image(u, &all)?))
        })
        .collect();
    let mut images: Vec<&String> = member_images.values().collect();
    images.sort();
    images.dedup();
    let image_ports: HashMap<&String, Vec<ports::ExposedPort>> = images
        .iter()
        .copied()
        .zip(join_all(images.iter().map(|i| imageinfo::exposed_ports(i))).await)
        .filter_map(|(i, p)| Some((i, p?)))
        .collect();
    let exposed: HashMap<&str, Vec<ports::ExposedPort>> = member_files
        .iter()
        .filter_map(|f| unit_refs.iter().find(|u| &u.file_name == f))
        .map(|u| {
            let mut e = ports::declared_exposed(u);
            if let Some(p) = member_images
                .get(u.file_name.as_str())
                .and_then(|i| image_ports.get(i))
            {
                e.extend(p);
            }
            (u.file_name.as_str(), e)
        })
        .collect();
    // Members whose image podman couldn't inspect -- almost always "not
    // pulled yet" -- and that a pull could fix (not a local `.build`).
    let unpulled: HashMap<&str, &String> = member_images
        .iter()
        .filter(|(f, i)| !image_ports.contains_key(i) && !builds_locally(f, &unit_refs))
        .map(|(f, i)| (*f, i))
        .collect();

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
            // the container side) listens on the container port. Narrow to
            // the members that declare that port; if none does, any of them
            // might, so keep them all.
            let members = if unit.kind == UnitKind::Pod {
                let all_members = refs::pod_members(unit, &unit_refs);
                let serving: Vec<&String> = all_members
                    .iter()
                    .filter(|f| exposed.get(f.as_str()).is_some_and(|e| ports::serves(e, m)))
                    .collect();
                let matched = !serving.is_empty();
                let shown: Vec<&String> = if matched {
                    serving
                } else {
                    all_members.iter().collect()
                };
                Some(PodTargets {
                    containers: shown
                        .into_iter()
                        .filter_map(|f| {
                            let mut r = unit_ref(f)?;
                            r.unpulled = unpulled.get(f.as_str()).map(|i| i.to_string());
                            Some(r)
                        })
                        .collect(),
                    matched,
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

    Ok(render(&rows))
}

/// Whether a container's `Image=` names a `.build` quadlet -- a locally
/// built image there's nothing to pull for.
fn builds_locally(file_name: &str, units: &[QuadletUnit]) -> bool {
    units
        .iter()
        .find(|u| u.file_name == file_name)
        .and_then(|u| u.section("Container")?.get("Image"))
        .is_some_and(|i| i.trim().ends_with(".build"))
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
    Ok(with_rows(&state, |rows| {
        templates::ports::ports_page(rows, &csrf, health)
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
    Ok(with_rows(&state, |rows| {
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
    let image = all
        .iter()
        .find(|u| u.kind == UnitKind::Container && u.file_name == form.file_name)
        .filter(|_| !builds_locally(&form.file_name, &all))
        .and_then(|u| refs::container_image(u, &all))
        .ok_or_else(|| AppError::NotFound(format!("no pullable image for {}", form.file_name)))?;
    let error = imageinfo::pull(&image).await.err();
    let csrf = csrf(&session).await;
    let pull_error = error.as_deref().map(|e| (form.file_name.as_str(), e));
    Ok(with_rows(&state, |rows| {
        templates::ports::ports_rows(rows, &csrf, pull_error)
    })
    .await?)
}
