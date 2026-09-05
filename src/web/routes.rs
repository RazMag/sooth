use axum::http::StatusCode;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::auth;
use crate::config::AppState;

use super::handlers::{dashboard, logs, unit_actions, unit_detail, unit_edit};
use super::sse;

/// Assembles the whole app: a `protected` sub-router (everything requiring a
/// signed-in session) merged with a small `public` one (login, static
/// assets, health check), with the session layer wrapping both -- login
/// itself needs a session to write "authenticated" into -- and the auth
/// middleware applied only to `protected`.
pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/", get(dashboard::dashboard))
        .route("/units/new", get(unit_edit::new_form))
        .route("/units", post(unit_edit::create))
        .route("/units/{file_name}", get(unit_detail::detail))
        .route("/units/{file_name}/status", get(unit_detail::status_fragment))
        .route("/units/{file_name}/start", post(unit_actions::start))
        .route("/units/{file_name}/stop", post(unit_actions::stop))
        .route("/units/{file_name}/restart", post(unit_actions::restart))
        .route("/units/{file_name}/enable", post(unit_actions::enable))
        .route("/units/{file_name}/disable", post(unit_actions::disable))
        .route("/units/{file_name}/edit", get(unit_edit::edit_form).post(unit_edit::edit_submit))
        .route("/units/{file_name}/delete", post(unit_edit::delete))
        .route("/units/{file_name}/logs", get(logs::logs_page))
        .route("/units/{file_name}/logs/stream", get(logs::logs_stream))
        .route("/events", get(sse::events_stream))
        .route("/logout", post(auth::session::logout))
        .route_layer(middleware::from_fn(auth::middleware::require_auth));

    let public = Router::new()
        .route("/login", get(auth::session::login_page).post(auth::session::login_submit))
        .route("/healthz", get(healthz))
        .nest_service("/static", ServeDir::new("static"));

    Router::new()
        .merge(protected)
        .merge(public)
        .fallback(not_found)
        .layer(TraceLayer::new_for_http())
        .layer(auth::session::layer(state.config.cookie_secure, state.config.session_idle_timeout_secs))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}

async fn not_found(uri: axum::http::Uri) -> crate::error::PageError {
    crate::error::PageError(crate::error::AppError::NotFound(uri.path().to_string()))
}
