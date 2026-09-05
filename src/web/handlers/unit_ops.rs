//! Start/stop/restart/enable/disable -- exactly one implementation each,
//! mounted at every section's URL prefix (see `routes::mount_unit_routes`).
//! None of this differs by kind, so there's nothing to specialize. Live
//! status refresh is handled entirely by the SSE `status-{service}` swap, so
//! there is no manual status-fragment route.

use axum::Form;
use axum::extract::{Path, State};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::FragmentError;
use crate::web::{core, templates};

#[derive(Deserialize)]
pub struct ActionForm {
    csrf_token: String,
}

macro_rules! action_handler {
    ($name:ident, $action:literal) => {
        pub async fn $name(
            State(state): State<AppState>,
            session: Session,
            Path(file_name): Path<String>,
            Form(form): Form<ActionForm>,
        ) -> Result<maud::Markup, FragmentError> {
            let outcome =
                core::execute_action(&state, &session, &file_name, &form.csrf_token, $action)
                    .await?;
            Ok(templates::status_badge(
                &outcome.unit.service_name(),
                &outcome.status,
            ))
        }
    };
}

action_handler!(start, "start");
action_handler!(stop, "stop");
action_handler!(restart, "restart");
action_handler!(enable, "enable");
action_handler!(disable, "disable");
