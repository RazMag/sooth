//! The host environment the systemd user manager hands to the quadlet
//! generator -- the variables a quadlet file can reference as `${NAME}`.
//!
//! sooth manages its own slice of this through an `environment.d` drop-in
//! (see `crate::hostenv`); every change is also pushed to the running
//! manager over D-Bus so it applies without a re-login. Variables configured
//! in other files are shown read-only.

use axum::Form;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::hostenv;
use crate::web::templates;
use crate::web::templates::environment::EnvironmentPage;

#[derive(Deserialize)]
pub struct FlashQuery {
    /// `?set=1` / `?unset=1` after a successful redirect; `&live=0` when the
    /// file was written but the D-Bus push to the running manager failed.
    set: Option<u8>,
    unset: Option<u8>,
    live: Option<u8>,
}

pub async fn index(
    State(state): State<AppState>,
    session: Session,
    Query(flash): Query<FlashQuery>,
) -> Result<Response, PageError> {
    let notice = match (flash.set.is_some(), flash.unset.is_some(), flash.live) {
        (true, _, Some(0)) => Some(
            "Saved to the environment.d drop-in. It could not be applied to the running \
             manager — it will take effect on your next login.",
        ),
        (_, true, Some(0)) => Some(
            "Removed from the environment.d drop-in. The running manager still has it — \
             it clears on your next login.",
        ),
        (true, _, _) => Some("Variable set."),
        (_, true, _) => Some("Variable removed."),
        _ => None,
    };
    Ok(render(&state, &session, None, notice, None).await)
}

#[derive(Deserialize)]
pub struct AddForm {
    csrf_token: String,
    name: String,
    value: String,
}

pub async fn add(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<AddForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    let name = form.name.trim();
    let value = form.value.as_str();
    let prefill = (name, value);

    if !hostenv::valid_name(name) {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            render(
                &state,
                &session,
                Some(prefill),
                None,
                Some(
                    "Name must start with a letter or underscore and contain only letters, \
                     digits, and underscores.",
                ),
            )
            .await,
        )
            .into_response());
    }
    if !hostenv::valid_value(value) {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            render(
                &state,
                &session,
                Some(prefill),
                None,
                Some("Value must be a single line."),
            )
            .await,
        )
            .into_response());
    }

    hostenv::set(name, value).map_err(|e| AppError::Internal(e.into()))?;

    let live_ok = state
        .systemd
        .set_environment(&[format!("{name}={value}")])
        .await
        .inspect_err(|e| tracing::warn!(var = name, error = %e, "live set-environment failed"))
        .is_ok();
    tracing::info!(var = name, live_ok, "host env var set");

    Ok(Redirect::to(redirect_target("set", live_ok)).into_response())
}

#[derive(Deserialize)]
pub struct RemoveForm {
    csrf_token: String,
    name: String,
}

pub async fn remove(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<RemoveForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let name = form.name.trim();

    hostenv::unset(name).map_err(|e| AppError::Internal(e.into()))?;

    let live_ok = state
        .systemd
        .unset_environment(&[name.to_string()])
        .await
        .inspect_err(|e| tracing::warn!(var = name, error = %e, "live unset-environment failed"))
        .is_ok();
    tracing::info!(var = name, live_ok, "host env var removed");

    Ok(Redirect::to(redirect_target("unset", live_ok)).into_response())
}

fn redirect_target(action: &str, live_ok: bool) -> &'static str {
    match (action, live_ok) {
        ("set", true) => "/environment?set=1",
        ("set", false) => "/environment?set=1&live=0",
        ("unset", true) => "/environment?unset=1",
        _ => "/environment?unset=1&live=0",
    }
}

async fn render(
    state: &AppState,
    session: &Session,
    prefill: Option<(&str, &str)>,
    notice: Option<&str>,
    error: Option<&str>,
) -> Response {
    let csrf = crate::auth::csrf::current(session)
        .await
        .unwrap_or_default();
    let configured = hostenv::load().unwrap_or_default();
    let live = state.systemd.environment().await.unwrap_or_default();
    let managed_file = hostenv::managed_file()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    templates::environment::page(EnvironmentPage {
        csrf: &csrf,
        configured: &configured,
        live: &live,
        managed_file: &managed_file,
        prefill,
        notice,
        error,
        health: state.health,
    })
    .into_response()
}
