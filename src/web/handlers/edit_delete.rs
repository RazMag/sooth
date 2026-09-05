//! Raw-textarea edit and delete -- exactly one implementation each, mounted
//! at every section's URL prefix. Edit stays raw for every kind, including
//! Containers and Pods (only *create* gets a structured form); delete is
//! identical everywhere.

use axum::Form;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::quadlet::{discovery, naming};
use crate::web::{core, templates};

pub async fn edit_form(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, PageError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    Ok(templates::generic::edit_unit_page(&unit, &csrf, None))
}

#[derive(Deserialize)]
pub struct EditForm {
    csrf_token: String,
    contents: String,
}

pub async fn edit_submit(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
    Form(form): Form<EditForm>,
) -> Result<Response, PageError> {
    match core::edit_unit(
        &state,
        &session,
        &form.csrf_token,
        &file_name,
        &form.contents,
    )
    .await
    {
        Ok(unit) => Ok(Redirect::to(&core::unit_url(&unit)).into_response()),
        Err(AppError::Quadlet(e)) if e.is_client_error() => {
            let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
            let csrf = crate::auth::csrf::current(&session)
                .await
                .unwrap_or_default();
            Ok((
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                templates::generic::edit_unit_page(&unit, &csrf, Some(&e.to_string())),
            )
                .into_response())
        }
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize)]
pub struct DeleteForm {
    csrf_token: String,
}

pub async fn delete(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
    Form(form): Form<DeleteForm>,
) -> Result<impl IntoResponse, PageError> {
    core::delete_unit(&state, &session, &form.csrf_token, &file_name).await?;
    let redirect = match naming::kind_of(&file_name) {
        Some(kind) => core::section_path(kind),
        None => "/units",
    };
    Ok(Redirect::to(redirect))
}
