use axum::extract::State;
use axum::response::IntoResponse;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{FragmentError, PageError};
use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;
use crate::web::core;
use crate::web::templates::services::Stats;
use crate::web::templates::{self};

const KINDS: &[UnitKind] = &[UnitKind::Container, UnitKind::Pod];

fn compute_stats(units: &[(QuadletUnit, UnitStatus)]) -> Stats {
    let running = units.iter().filter(|(_, s)| s.is_active()).count();
    let failed = units.iter().filter(|(_, s)| s.is_failed()).count();
    Stats {
        total: units.len(),
        running,
        failed,
    }
}

pub async fn page(
    State(state): State<AppState>,
    session: Session,
) -> Result<impl IntoResponse, PageError> {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let units = core::load_units_for_kinds(&state, KINDS).await?;
    let stats = compute_stats(&units);
    Ok(templates::services::services_page(&units, &stats, &csrf))
}

pub async fn rows(
    State(state): State<AppState>,
    session: Session,
) -> Result<impl IntoResponse, FragmentError> {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let units = core::load_units_for_kinds(&state, KINDS).await?;
    Ok(templates::list::list_rows(
        &templates::services::SPEC,
        &units,
        &csrf,
    ))
}

pub async fn counts(State(state): State<AppState>) -> Result<impl IntoResponse, FragmentError> {
    let units = core::load_units_for_kinds(&state, KINDS).await?;
    Ok(templates::services::counts_fragment(&compute_stats(&units)))
}
