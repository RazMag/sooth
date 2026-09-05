use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::Form;
use serde::Deserialize;
use tower_sessions::Session;
use tracing::Instrument;

use crate::config::AppState;
use crate::error::{AppError, FragmentError};
use crate::events::DashboardEvent;
use crate::quadlet::discovery;
use crate::web::templates;

#[derive(Deserialize)]
pub struct ActionForm {
    csrf_token: String,
}

/// Shared body for the five action routes below: verify CSRF, resolve the
/// quadlet file to its systemd service name, run the systemd action inside a
/// span carrying unit/action context (so every log line during it is
/// automatically tagged), broadcast the resulting status to any open
/// dashboard tabs, and return the freshly rendered status badge.
async fn run_action(
    state: &AppState,
    session: &Session,
    file_name: &str,
    csrf_token: &str,
    action: &'static str,
) -> Result<maud::Markup, FragmentError> {
    if !crate::auth::csrf::verify(session, csrf_token).await {
        return Err(FragmentError(AppError::Csrf));
    }
    let unit = discovery::load_by_name(&state.quadlet_dir, file_name)?;
    let service = unit.service_name();

    let span = tracing::info_span!("unit_action", unit = %service, action);
    async {
        let result = match action {
            "start" => state.systemd.start(&service).await,
            "stop" => state.systemd.stop(&service).await,
            "restart" => state.systemd.restart(&service).await,
            "enable" => state.systemd.enable(&service).await,
            "disable" => state.systemd.disable(&service).await,
            _ => unreachable!("action is one of the fixed route names below"),
        };
        result.map_err(FragmentError::from)
    }
    .instrument(span)
    .await?;

    let status = state.systemd.status(&service).await?;
    let _ = state.events.send(DashboardEvent::Status { service: service.clone(), status: status.clone() });
    Ok(templates::status_badge(&service, &status))
}

macro_rules! action_handler {
    ($name:ident, $action:literal) => {
        pub async fn $name(
            State(state): State<AppState>,
            session: Session,
            Path(file_name): Path<String>,
            Form(form): Form<ActionForm>,
        ) -> Result<impl IntoResponse, FragmentError> {
            run_action(&state, &session, &file_name, &form.csrf_token, $action).await
        }
    };
}

action_handler!(start, "start");
action_handler!(stop, "stop");
action_handler!(restart, "restart");
action_handler!(enable, "enable");
action_handler!(disable, "disable");
