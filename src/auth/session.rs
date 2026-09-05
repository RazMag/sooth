use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use time::Duration;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, MemoryStore, Session, SessionManagerLayer};
use tracing::{info, warn};

use crate::config::AppState;
use crate::error::PageError;
use crate::web::templates;

const AUTH_KEY: &str = "authenticated";

/// Builds the session middleware layer: an in-memory store (no database --
/// sessions are lost on restart, which just means re-logging in) with cookie
/// flags set for a single-operator dashboard: `HttpOnly` always, `SameSite=Strict`
/// to block cross-site requests outright, and `Secure` left configurable since
/// this app has no built-in TLS (see `Config::cookie_secure` docs).
pub fn layer(secure: bool, idle_timeout_secs: u64) -> SessionManagerLayer<MemoryStore> {
    let store = MemoryStore::default();
    SessionManagerLayer::new(store)
        .with_secure(secure)
        .with_http_only(true)
        .with_same_site(SameSite::Strict)
        .with_expiry(Expiry::OnInactivity(Duration::seconds(idle_timeout_secs as i64)))
}

pub async fn is_authenticated(session: &Session) -> bool {
    session.get::<bool>(AUTH_KEY).await.ok().flatten().unwrap_or(false)
}

#[derive(Deserialize)]
pub struct LoginForm {
    password: String,
}

pub async fn login_page() -> impl IntoResponse {
    templates::login_page(None)
}

pub async fn login_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> Result<Response, PageError> {
    use argon2::password_hash::PasswordVerifier;

    let hash = argon2::PasswordHash::new(&state.config.auth_password_hash)
        .map_err(|e| anyhow::anyhow!("stored SOOTH_AUTH_PASSWORD_HASH is not a valid argon2 hash: {e}"))?;
    let valid = argon2::Argon2::default().verify_password(form.password.as_bytes(), &hash).is_ok();

    if !valid {
        warn!("failed login attempt");
        return Ok(templates::login_page(Some("Invalid password")).into_response());
    }

    // Regenerate the session id on privilege change to prevent session fixation.
    session.cycle_id().await.map_err(|e| anyhow::anyhow!("session error: {e}"))?;
    session.insert(AUTH_KEY, true).await.map_err(|e| anyhow::anyhow!("session error: {e}"))?;
    crate::auth::csrf::store(&session, &crate::auth::csrf::generate())
        .await
        .map_err(|e| anyhow::anyhow!("session error: {e}"))?;

    info!("login succeeded");
    Ok(Redirect::to("/").into_response())
}

pub async fn logout(session: Session) -> impl IntoResponse {
    let _ = session.flush().await;
    Redirect::to("/login")
}
