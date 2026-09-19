//! The Settings page's "Updates" status: current version and
//! "Check now"/"Download update" actions, re-fetched on
//! `sse:self-update-changed`. The editable settings (mode, poll interval,
//! repo) live in the main settings form now -- see
//! `crate::web::handlers::settings::save`, which both persists them to the
//! config file and applies them live via `AppState.self_update`
//! (`selfupdate::SelfUpdateManager`) in the same request, unlike the rest of
//! Settings which needs a restart. Once a download lands on
//! `ReadyToRestart`, the "Install and restart" button posts to the existing
//! `settings::restart` action directly -- installing a downloaded update
//! isn't a distinct self-update action, it's just the ordinary restart.

use axum::Form;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, FragmentError};
use crate::web::templates;

/// The status fragment only -- re-fetched by the page on
/// `sse:self-update-changed`, the self-update analogue of `gitsync::rows`.
pub async fn card(State(state): State<AppState>, session: Session) -> impl IntoResponse {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    templates::selfupdate::status_fragment(&state.self_update.snapshot(), &csrf)
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
