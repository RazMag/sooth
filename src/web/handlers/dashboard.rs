use axum::extract::State;
use axum::response::IntoResponse;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::quadlet::{discovery, QuadletUnit};
use crate::systemd::UnitStatus;
use crate::web::templates;

pub async fn dashboard(State(state): State<AppState>, session: Session) -> Result<impl IntoResponse, PageError> {
    let csrf = crate::auth::csrf::current(&session).await.unwrap_or_default();
    let units = load_units_with_status(&state).await?;
    Ok(templates::dashboard_page(&units, &csrf, &state.quadlet_dir.display().to_string()))
}

pub(crate) async fn load_units_with_status(
    state: &AppState,
) -> Result<Vec<(QuadletUnit, UnitStatus)>, AppError> {
    let units = discovery::load_all(&state.quadlet_dir)?;
    let mut out = Vec::with_capacity(units.len());
    for unit in units {
        let status = if unit.is_template() {
            UnitStatus::not_found()
        } else {
            state.systemd.status(&unit.service_name()).await?
        };
        out.push((unit, status));
    }
    Ok(out)
}

/// Re-renders just the unit rows, for the `units-changed` SSE broadcast.
/// Returns `None` (rather than erroring the whole stream) if the re-render
/// fails -- the next successful event will catch the UI back up.
pub(crate) async fn render_unit_rows(state: &AppState, csrf: &str) -> Option<String> {
    match load_units_with_status(state).await {
        Ok(units) => Some(templates::unit_rows(&units, csrf).into_string()),
        Err(e) => {
            tracing::warn!(error = %e, "failed to re-render unit list for SSE update");
            None
        }
    }
}
