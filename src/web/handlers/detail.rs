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
use crate::web::templates;

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

    Ok(match unit.kind {
        UnitKind::Container => templates::containers::detail_page(
            &unit,
            &status,
            &csrf,
            &known_groups,
            state.health.get(),
        ),
        UnitKind::Pod => {
            let member_count = all
                .iter()
                .filter(|c| c.kind == UnitKind::Container)
                .filter(|c| {
                    c.section("Container").and_then(|s| s.get("Pod"))
                        == Some(unit.file_name.as_str())
                })
                .count();
            templates::pods::detail_page(
                &unit,
                &status,
                &csrf,
                member_count,
                &known_groups,
                state.health.get(),
            )
        }
        UnitKind::Volume => {
            let used_by = refs::consumers_of(&unit, &all);
            templates::volumes::detail_page(
                &unit,
                &status,
                &csrf,
                &all,
                &used_by,
                &known_groups,
                state.health.get(),
            )
        }
        UnitKind::Network => {
            let used_by = refs::consumers_of(&unit, &all);
            templates::networks::detail_page(
                &unit,
                &status,
                &csrf,
                &all,
                &used_by,
                &known_groups,
                state.health.get(),
            )
        }
        UnitKind::Image | UnitKind::Build => {
            templates::images::detail_page(&unit, &status, &csrf, &known_groups, state.health.get())
        }
        UnitKind::Kube => templates::generic::detail_page(
            &unit,
            &status,
            &csrf,
            &known_groups,
            state.health.get(),
        ),
    })
}
