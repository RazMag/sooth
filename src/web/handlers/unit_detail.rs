use axum::extract::{Path, State};
use axum::response::IntoResponse;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{FragmentError, PageError};
use crate::quadlet::discovery;
use crate::web::templates;

pub async fn detail(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, PageError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    let status = state.systemd.status(&unit.service_name()).await?;
    let csrf = crate::auth::csrf::current(&session).await.unwrap_or_default();
    Ok(templates::unit_detail_page(&unit, &status, &csrf))
}

/// A standalone status-badge fragment, for a manual refresh button as a
/// fallback to the SSE live-update stream.
pub async fn status_fragment(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, FragmentError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    let service = unit.service_name();
    let status = state.systemd.status(&service).await?;
    Ok(templates::status_badge(&service, &status))
}
