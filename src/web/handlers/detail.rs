//! The single detail-page dispatcher, mounted at every section's prefix. It
//! loads the unit and its status once, then picks the right template based
//! on `unit.kind` -- correct regardless of which prefix was actually hit,
//! since the dispatch key is the loaded unit's real kind, not the URL.

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::PageError;
use crate::quadlet::{UnitKind, discovery, refs};
use crate::web::templates::detail::DetailCtx;
use crate::web::{core, templates};

pub async fn show(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, PageError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    let status = state.systemd.status(&unit.service_name()).await?;
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();

    // One enumeration, reused for cross-unit references; every group directory
    // (incl. empty ones) feeds the group picker.
    let all = discovery::load_all(&state.quadlet_dir)?;
    let known_groups = discovery::list_groups(&state.quadlet_dir);
    let stores = core::RefStores::load(&state, [&unit]).await;
    let host_vars: Vec<(String, Option<bool>)> = refs::env_refs(&unit)
        .into_iter()
        .map(|n| {
            let present = stores.env.as_ref().map(|e| e.contains(&n));
            (n, present)
        })
        .collect();
    let ctx = DetailCtx {
        known_groups: &known_groups,
        health: state.health.get(),
        host_vars: &host_vars,
    };

    Ok(match unit.kind {
        UnitKind::Container => {
            let secrets: Vec<(String, Option<bool>)> = refs::secret_refs(&unit)
                .into_iter()
                .map(|n| {
                    let present = stores.secrets.as_ref().map(|e| e.contains(&n));
                    (n, present)
                })
                .collect();
            templates::containers::detail_page(&unit, &status, &csrf, &secrets, &ctx)
        }
        UnitKind::Pod => {
            let members = refs::pod_members(&unit, &all);
            let (networks, volumes) = refs::pod_own_refs(&unit, &all);
            templates::pods::detail_page(
                &unit, &status, &csrf, &all, &members, &networks, &volumes, &ctx,
            )
        }
        UnitKind::Volume => {
            let used_by = refs::consumers_of(&unit, &all);
            templates::volumes::detail_page(&unit, &status, &csrf, &all, &used_by, &ctx)
        }
        UnitKind::Network => {
            let used_by = refs::consumers_of(&unit, &all);
            templates::networks::detail_page(&unit, &status, &csrf, &all, &used_by, &ctx)
        }
        UnitKind::Image | UnitKind::Build => {
            templates::images::detail_page(&unit, &status, &csrf, &ctx)
        }
        UnitKind::Kube => templates::generic::detail_page(&unit, &status, &csrf, &ctx),
    })
}
