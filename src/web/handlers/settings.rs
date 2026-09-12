//! View and change persisted configuration. Writes go straight to the
//! resolved config TOML file and require a restart to take effect -- no
//! runtime-mutable `AppState`, no re-pointing the filesystem watcher or
//! D-Bus session on the fly. Simpler and safer for what's really a rarely-
//! touched settings screen.

use std::net::SocketAddr;

use axum::Form;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, FragmentError, PageError};
use crate::web::templates::settings::{EnvLocks, FormValues};
use crate::web::templates::{self};

#[derive(Deserialize)]
pub struct SavedQuery {
    saved: Option<String>,
}

pub async fn page(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<SavedQuery>,
) -> impl IntoResponse {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let values = FormValues::from_config(&state.config);
    let message = match query.saved.as_deref() {
        Some("password") => {
            Some("Password saved. Restart sooth for the new password to take effect.")
        }
        Some(_) => Some("Saved. Restart sooth for changes to take effect."),
        None => None,
    };
    templates::settings::page(
        &values,
        &EnvLocks::detect(),
        &state.config_path.display().to_string(),
        &csrf,
        message,
        None,
        state.health.get(),
    )
}

/// Re-render the settings page with a 422 and an error banner. Shared by
/// every rejected submission on either form.
async fn render_error(
    state: &AppState,
    session: &Session,
    values: &FormValues,
    msg: &str,
) -> Response {
    let csrf = crate::auth::csrf::current(session)
        .await
        .unwrap_or_default();
    (
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        templates::settings::page(
            values,
            &EnvLocks::detect(),
            &state.config_path.display().to_string(),
            &csrf,
            None,
            Some(msg),
            state.health.get(),
        ),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct SettingsForm {
    csrf_token: String,
    bind_addr: String,
    quadlet_dir: String,
    #[serde(default)]
    cookie_secure: Option<String>,
    log_filter: String,
    session_idle_timeout_secs: String,
}

pub async fn save(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<SettingsForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    // Reconstruct the form so a rejected submission redisplays exactly as
    // typed (same pattern as the quadlet create/edit forms).
    let entered = || FormValues {
        bind_addr: form.bind_addr.clone(),
        quadlet_dir: form.quadlet_dir.clone(),
        cookie_secure: form.cookie_secure.is_some(),
        log_filter: form.log_filter.clone(),
        session_idle_timeout_secs: form.session_idle_timeout_secs.clone(),
    };

    let Ok(bind_addr) = form.bind_addr.trim().parse::<SocketAddr>() else {
        return Ok(render_error(
            &state,
            &session,
            &entered(),
            "Bind address must be a valid host:port, e.g. 127.0.0.1:8420",
        )
        .await);
    };
    let Ok(session_idle_timeout_secs) = form.session_idle_timeout_secs.trim().parse::<u64>() else {
        return Ok(render_error(
            &state,
            &session,
            &entered(),
            "Session idle timeout must be a whole number of seconds",
        )
        .await);
    };
    let quadlet_dir = form.quadlet_dir.trim();
    if quadlet_dir.is_empty() {
        return Ok(render_error(
            &state,
            &session,
            &entered(),
            "Quadlet directory must not be empty",
        )
        .await);
    }

    let updates = [
        ("bind_addr", toml::Value::String(bind_addr.to_string())),
        ("quadlet_dir", toml::Value::String(quadlet_dir.to_string())),
        (
            "cookie_secure",
            toml::Value::Boolean(form.cookie_secure.is_some()),
        ),
        (
            "log_filter",
            toml::Value::String(form.log_filter.trim().to_string()),
        ),
        (
            "session_idle_timeout_secs",
            toml::Value::Integer(session_idle_timeout_secs as i64),
        ),
    ];

    crate::config::patch_config_toml(&state.config_path, &updates).map_err(|e| {
        AppError::Internal(anyhow::anyhow!(
            "failed to write {}: {e}",
            state.config_path.display()
        ))
    })?;

    tracing::info!(path = %state.config_path.display(), "settings saved");
    Ok(Redirect::to("/settings?saved=settings").into_response())
}

#[derive(Deserialize)]
pub struct ChangePasswordForm {
    csrf_token: String,
    current_password: String,
    new_password: String,
    confirm_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<ChangePasswordForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    let values = FormValues::from_config(&state.config);
    let fail = |msg: &'static str| render_error(&state, &session, &values, msg);

    if EnvLocks::detect().auth_password_hash {
        return Ok(fail(
            "The password is set by the SOOTH_AUTH_PASSWORD_HASH environment variable; \
             change it there and restart.",
        )
        .await);
    }
    if !crate::auth::verify_password(&state.config.auth_password_hash, &form.current_password) {
        return Ok(fail("Current password is incorrect.").await);
    }
    if form.new_password != form.confirm_password {
        return Ok(fail("New password and confirmation do not match.").await);
    }
    if form.new_password.chars().count() < 8 {
        return Ok(fail("New password must be at least 8 characters.").await);
    }

    let hash = crate::auth::hash_password(&form.new_password).map_err(AppError::Internal)?;
    crate::config::patch_config_toml(
        &state.config_path,
        &[("auth_password_hash", toml::Value::String(hash))],
    )
    .map_err(|e| {
        AppError::Internal(anyhow::anyhow!(
            "failed to write {}: {e}",
            state.config_path.display()
        ))
    })?;

    tracing::info!(path = %state.config_path.display(), "password changed via settings");
    Ok(Redirect::to("/settings?saved=password").into_response())
}

#[derive(Deserialize)]
pub struct RestartForm {
    csrf_token: String,
}

/// Signal `main` to wind down and re-exec the binary (see `AppState::restart`
/// and `main::reexec`). Returns a small "restarting…" page that polls
/// `/healthz` and bounces back to the dashboard once the fresh process is up.
pub async fn restart(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<RestartForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    tracing::info!("restart requested from the settings page");
    state.restart.notify_one();
    Ok(templates::settings::restarting_page().into_response())
}

#[derive(Deserialize)]
pub struct RefreshHealthForm {
    csrf_token: String,
}

/// The System card's Refresh button (htmx `outerHTML` swap of `#health-card`
/// only, not a full page reload). Re-runs both host-dependency checks and
/// stores the result back into `AppState::health`, so the banner and the
/// sidebar's notice dot on every other page pick up the change immediately
/// too -- not just this card.
pub async fn refresh_health(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<RefreshHealthForm>,
) -> Result<impl IntoResponse, FragmentError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    Ok(templates::settings::health_card(state.health.refresh()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn hash_then_verify_round_trips() {
        let hash = crate::auth::hash_password("correct horse battery staple").unwrap();
        assert!(crate::auth::verify_password(
            &hash,
            "correct horse battery staple"
        ));
        assert!(!crate::auth::verify_password(&hash, "wrong password"));
    }
}
