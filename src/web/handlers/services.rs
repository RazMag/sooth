use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{FragmentError, PageError};
use crate::quadlet::ports::SplitPorts;
use crate::quadlet::{QuadletUnit, UnitKind, discovery};
use crate::systemd::UnitStatus;
use crate::web::core;
use crate::web::handlers::list::{ViewQuery, resolve_list_view};
use crate::web::templates::list::ListContext;
use crate::web::templates::services::Stats;
use crate::web::templates::{self};

const KINDS: &[UnitKind] = templates::services::SPEC.kinds;

pub fn compute_stats(units: &[(QuadletUnit, UnitStatus)]) -> Stats {
    let running = units.iter().filter(|(_, s)| s.is_active()).count();
    let failed = units.iter().filter(|(_, s)| s.is_failed()).count();
    Stats {
        total: units.len(),
        running,
        failed,
    }
}

/// Every listed pod's published ports, split onto the members serving them.
async fn split_ports(
    units: &[(QuadletUnit, UnitStatus)],
    all: &[QuadletUnit],
) -> HashMap<String, SplitPorts> {
    let pods: Vec<&QuadletUnit> = units
        .iter()
        .map(|(u, _)| u)
        .filter(|u| u.kind == UnitKind::Pod)
        .collect();
    core::split_pod_ports(&pods, all).await
}

pub async fn page(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<ViewQuery>,
) -> Result<impl IntoResponse, PageError> {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let (units, all) = core::load_units_and_siblings(&state, KINDS).await?;
    let known = discovery::list_groups(&state.quadlet_dir);
    let synced = state.git_sync.synced_groups();
    let missing = core::missing_refs(&state, &units).await;
    let tags = core::newer_tags(&state, &units, &all);
    let ports = split_ports(&units, &all).await;
    let stats = compute_stats(&units);
    let view = resolve_list_view(&session, query.view.as_deref()).await;
    Ok(templates::services::services_page(
        &units,
        &stats,
        &csrf,
        &all,
        &ListContext {
            known: &known,
            synced: &synced,
            missing: &missing,
            tags: &tags,
            ports: Some(&ports),
            view,
        },
        state.health.get(),
    ))
}

pub async fn rows(
    State(state): State<AppState>,
    session: Session,
) -> Result<impl IntoResponse, FragmentError> {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let (units, all) = core::load_units_and_siblings(&state, KINDS).await?;
    let known = discovery::list_groups(&state.quadlet_dir);
    let synced = state.git_sync.synced_groups();
    let missing = core::missing_refs(&state, &units).await;
    let tags = core::newer_tags(&state, &units, &all);
    let ports = split_ports(&units, &all).await;
    let view = resolve_list_view(&session, None).await;
    Ok(templates::list::list_rows(
        &templates::services::SPEC,
        &units,
        &csrf,
        &all,
        &ListContext {
            known: &known,
            synced: &synced,
            missing: &missing,
            tags: &tags,
            ports: Some(&ports),
            view,
        },
    ))
}

pub async fn counts(State(state): State<AppState>) -> Result<impl IntoResponse, FragmentError> {
    let units = core::load_units_for_kinds(&state, KINDS).await?;
    Ok(templates::services::counts_fragment(&compute_stats(&units)))
}

#[derive(serde::Deserialize)]
pub struct BulkForm {
    csrf_token: String,
    /// `start`, `stop` or `restart` -- the bulk bar button pressed.
    action: String,
    /// The selected units' file names, comma-separated (filled in by
    /// `frontend/bulk.js`).
    units: String,
}

/// The bulk bar: one action on every selected unit, in turn. Answers with
/// a one-line result for the bar (always 200 -- htmx drops other bodies);
/// the rows' badges update themselves over SSE.
pub async fn bulk(
    State(state): State<AppState>,
    session: Session,
    axum::Form(form): axum::Form<BulkForm>,
) -> impl IntoResponse {
    let (action, done): (&'static str, &str) = match form.action.as_str() {
        "start" => ("start", "Started"),
        "stop" => ("stop", "Stopped"),
        "restart" => ("restart", "Restarted"),
        _ => {
            return templates::services::bulk_result(
                0,
                &[("".into(), "unknown action".into())],
                "",
            );
        }
    };
    let mut ok = 0;
    let mut failed: Vec<(String, String)> = Vec::new();
    for name in form
        .units
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        match core::execute_action(&state, &session, name, &form.csrf_token, action).await {
            Ok(_) => ok += 1,
            Err(e) => failed.push((name.to_string(), e.0.to_string())),
        }
    }
    templates::services::bulk_result(ok, &failed, done)
}
