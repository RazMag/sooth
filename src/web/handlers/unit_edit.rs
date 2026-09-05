use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::events::DashboardEvent;
use crate::quadlet::{discovery, writer};
use crate::web::templates;

pub async fn new_form(session: Session) -> impl IntoResponse {
    let csrf = crate::auth::csrf::current(&session).await.unwrap_or_default();
    templates::new_unit_page(&csrf, None)
}

#[derive(Deserialize)]
pub struct CreateForm {
    csrf_token: String,
    file_name: String,
    contents: String,
}

pub async fn create(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CreateForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    match writer::write_atomic(&state.quadlet_dir, &form.file_name, &form.contents) {
        Ok(_) => {
            state.systemd.reload().await?;
            let _ = state.events.send(DashboardEvent::UnitsChanged);
            tracing::info!(file = form.file_name, "quadlet created");
            Ok(Redirect::to(&format!("/units/{}", form.file_name)).into_response())
        }
        Err(e) => {
            tracing::warn!(file = form.file_name, error = %e, "rejected new quadlet file");
            let csrf = crate::auth::csrf::current(&session).await.unwrap_or_default();
            Ok((StatusCode::UNPROCESSABLE_ENTITY, templates::new_unit_page(&csrf, Some(&e.to_string()))).into_response())
        }
    }
}

pub async fn edit_form(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
) -> Result<impl IntoResponse, PageError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    let csrf = crate::auth::csrf::current(&session).await.unwrap_or_default();
    Ok(templates::edit_unit_page(&unit, &csrf, None))
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
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    match writer::write_atomic(&state.quadlet_dir, &file_name, &form.contents) {
        Ok(_) => {
            state.systemd.reload().await?;
            let _ = state.events.send(DashboardEvent::UnitsChanged);
            tracing::info!(file = file_name, "quadlet edited");
            Ok(Redirect::to(&format!("/units/{file_name}")).into_response())
        }
        Err(e) => {
            tracing::warn!(file = file_name, error = %e, "rejected quadlet edit");
            let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
            let csrf = crate::auth::csrf::current(&session).await.unwrap_or_default();
            Ok((StatusCode::UNPROCESSABLE_ENTITY, templates::edit_unit_page(&unit, &csrf, Some(&e.to_string())))
                .into_response())
        }
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
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    writer::delete(&state.quadlet_dir, &file_name)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = file_name, "quadlet deleted");
    Ok(Redirect::to("/"))
}
