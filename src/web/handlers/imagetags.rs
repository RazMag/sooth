//! The Settings page's "Pinned image tags" status (see `crate::imagetags`):
//! the fragment re-fetched on `sse:image-tags-changed`, and "Check now".
//! The settings themselves save with the rest of the page
//! (`settings::save`).

use axum::Form;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, FragmentError};
use crate::web::templates;

pub async fn card(State(state): State<AppState>, session: Session) -> impl IntoResponse {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    templates::imagetags::status_fragment(&state.image_tags.snapshot(), &csrf)
}

#[derive(Deserialize)]
pub struct CsrfOnly {
    csrf_token: String,
}

/// "Check now": wakes the poll task for a full pass; the card follows its
/// progress over SSE, same shape as `selfupdate::check_now`.
pub async fn check_now(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CsrfOnly>,
) -> Result<impl IntoResponse, FragmentError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    state.image_tags.check_now();
    Ok(StatusCode::NO_CONTENT)
}
