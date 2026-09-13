//! The Settings page's "Updates" card: view/edit self-update settings and
//! drive "Check now"/"Download update". All mutation goes through
//! `AppState.self_update` (`selfupdate::SelfUpdateManager`), which persists
//! to the config file and (unlike the rest of Settings) applies live -- no
//! restart needed. Once a download lands on `ReadyToRestart`, the card's
//! "Install and restart" button posts to the existing
//! `settings::restart` action directly -- installing a downloaded update
//! isn't a distinct self-update action, it's just the ordinary restart.

use axum::Form;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, FragmentError};
use crate::selfupdate::{SelfUpdateConfig, UpdateMode};
use crate::web::templates;

/// The card fragment only -- re-fetched by the page on
/// `sse:self-update-changed`, the self-update analogue of `gitsync::rows`.
pub async fn card(State(state): State<AppState>, session: Session) -> impl IntoResponse {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    templates::selfupdate::card(
        &state.self_update.config_snapshot(),
        &state.self_update.snapshot(),
        &csrf,
    )
}

#[derive(Deserialize)]
pub struct SaveForm {
    csrf_token: String,
    mode: String,
    poll_interval_secs: String,
    repo: String,
}

/// Saves the settings form. htmx-only (the card's form posts with
/// `hx-target="#self-update-card"`), so a rejected submission re-renders
/// just the card with a 422 and an inline error, same convention as
/// `gitsync`'s edit disclosure.
pub async fn save(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<SaveForm>,
) -> Result<Response, FragmentError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    let render_error = |msg: &str| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            templates::selfupdate::card_with_error(
                &state.self_update.config_snapshot(),
                &state.self_update.snapshot(),
                &form.csrf_token,
                msg,
            ),
        )
            .into_response()
    };

    let Some(mode) = UpdateMode::parse(&form.mode) else {
        return Ok(render_error("Invalid mode"));
    };
    let Ok(poll_interval_secs) = form.poll_interval_secs.trim().parse::<u64>() else {
        return Ok(render_error("Invalid check interval"));
    };
    if poll_interval_secs < 60 {
        return Ok(render_error("Check interval must be at least 60 seconds"));
    }
    let repo = form.repo.trim().to_string();
    if crate::selfupdate::split_repo(&repo).is_err() {
        return Ok(render_error("Repository must look like \"owner/name\""));
    }

    let config = SelfUpdateConfig {
        mode,
        poll_interval_secs,
        repo,
    };
    match state.self_update.configure(config) {
        Ok(()) => Ok(templates::selfupdate::card(
            &state.self_update.config_snapshot(),
            &state.self_update.snapshot(),
            &form.csrf_token,
        )
        .into_response()),
        Err(e) => Ok(render_error(&e.to_string())),
    }
}

#[derive(Deserialize)]
pub struct CsrfOnly {
    csrf_token: String,
}

/// "Check now": always succeeds (there's only one target, and it always
/// exists), so this is a plain fire-and-forget wake, same shape as
/// `gitsync::sync_now`.
pub async fn check_now(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CsrfOnly>,
) -> Result<impl IntoResponse, FragmentError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    state.self_update.check_now();
    Ok(StatusCode::NO_CONTENT)
}

/// "Download update": the explicit download action for `Notify` mode, once
/// an update has been found. Downloads, verifies, and swaps the binary onto
/// disk, landing on `ReadyToRestart` -- it does not itself restart; that's
/// the card's separate "Install and restart" button, which posts straight to
/// `settings::restart`. Errors (no update recorded, or an attempt already in
/// flight) surface as a 422 via `FragmentError`; the button posts with
/// `hx-swap="none"` so the card's own SSE-driven refresh is what the user
/// actually sees update, same as `gitsync`'s "Sync now"/"Force resync".
pub async fn download_now(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CsrfOnly>,
) -> Result<impl IntoResponse, FragmentError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    state.self_update.download_now().await?;
    Ok(StatusCode::NO_CONTENT)
}
