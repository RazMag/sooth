//! View and change persisted configuration. Most fields are written straight
//! to the resolved config TOML file and require a restart to take effect --
//! no re-pointing the filesystem watcher or D-Bus session on the fly, simpler
//! and safer for what's really a rarely-touched settings screen. The
//! self-update fields (`mode`/`poll_interval_secs`/`repo`) are the one
//! exception: `save` also calls `AppState.self_update.configure(..)`, which
//! persists them and applies them live in the same request -- see
//! `crate::selfupdate`.

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
    let values = FormValues::from_config(&state.config, &state.self_update.config_snapshot());
    let message = match query.saved.as_deref() {
        Some("password") => {
            Some("Password saved. Restart sooth for the new password to take effect.")
        }
        Some("github_token") => Some("GitHub token saved. Restart sooth for it to take effect."),
        Some(_) => Some("Saved. Restart sooth for changes to take effect."),
        None => None,
    };
    let self_update_status = state.self_update.snapshot();
    templates::settings::page(
        &values,
        &EnvLocks::detect(),
        &state.config_path.display().to_string(),
        &csrf,
        message,
        None,
        templates::settings::LiveStatus {
            health: state.health.get(),
            self_update_status: &self_update_status,
        },
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
    let self_update_status = state.self_update.snapshot();
    (
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        templates::settings::page(
            values,
            &EnvLocks::detect(),
            &state.config_path.display().to_string(),
            &csrf,
            None,
            Some(msg),
            templates::settings::LiveStatus {
                health: state.health.get(),
                self_update_status: &self_update_status,
            },
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
    mode: String,
    poll_interval_secs: String,
    repo: String,
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
    // typed (same pattern as the quadlet create/edit forms). The GitHub
    // token isn't part of this form (it has its own, see
    // `save_github_token`), so its display fields come from the still-live
    // `state.config` rather than anything just submitted.
    let unchanged = FormValues::from_config(&state.config, &state.self_update.config_snapshot());
    let entered = || FormValues {
        bind_addr: form.bind_addr.clone(),
        quadlet_dir: form.quadlet_dir.clone(),
        cookie_secure: form.cookie_secure.is_some(),
        log_filter: form.log_filter.clone(),
        session_idle_timeout_secs: form.session_idle_timeout_secs.clone(),
        self_update_mode: form.mode.clone(),
        self_update_poll_interval_secs: form.poll_interval_secs.clone(),
        self_update_repo: form.repo.clone(),
        github_token_set: unchanged.github_token_set,
        github_token_last4: unchanged.github_token_last4.clone(),
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
    let Some(self_update_mode) = crate::selfupdate::UpdateMode::parse(&form.mode) else {
        return Ok(render_error(&state, &session, &entered(), "Invalid update-check mode").await);
    };
    let Ok(self_update_poll_interval_secs) = form.poll_interval_secs.trim().parse::<u64>() else {
        return Ok(render_error(&state, &session, &entered(), "Invalid check interval").await);
    };
    if self_update_poll_interval_secs < 60 {
        return Ok(render_error(
            &state,
            &session,
            &entered(),
            "Check interval must be at least 60 seconds",
        )
        .await);
    }
    let self_update_repo = form.repo.trim().to_string();
    if crate::selfupdate::split_repo(&self_update_repo).is_err() {
        return Ok(render_error(
            &state,
            &session,
            &entered(),
            "Repository must look like \"owner/name\"",
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

    if let Err(e) = state
        .self_update
        .configure(crate::selfupdate::SelfUpdateConfig {
            mode: self_update_mode,
            poll_interval_secs: self_update_poll_interval_secs,
            repo: self_update_repo,
        })
    {
        return Ok(render_error(&state, &session, &entered(), &e.to_string()).await);
    }

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

    let values = FormValues::from_config(&state.config, &state.self_update.config_snapshot());
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
pub struct GithubTokenForm {
    csrf_token: String,
    #[serde(default)]
    github_token: String,
}

/// Saves (or, given a blank field, clears) the GitHub access token used to
/// authenticate git-sync against private `https://github.com/...` remotes.
/// A separate form/route from the rest of Settings (like `change_password`)
/// so the token is never round-tripped back into the page as a prefilled
/// value -- the field the operator sees is always blank, whether or not one
/// is currently saved (see `FormValues::github_token_set`/`_last4`).
pub async fn save_github_token(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<GithubTokenForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    if EnvLocks::detect().github_token {
        let values = FormValues::from_config(&state.config, &state.self_update.config_snapshot());
        return Ok(render_error(
            &state,
            &session,
            &values,
            "The GitHub token is set by the SOOTH_GITHUB_TOKEN environment variable; \
             change it there and restart.",
        )
        .await);
    }

    crate::config::patch_config_toml(
        &state.config_path,
        &[(
            "github_token",
            toml::Value::String(form.github_token.trim().to_string()),
        )],
    )
    .map_err(|e| {
        AppError::Internal(anyhow::anyhow!(
            "failed to write {}: {e}",
            state.config_path.display()
        ))
    })?;

    tracing::info!(path = %state.config_path.display(), "github token saved via settings");
    Ok(Redirect::to("/settings?saved=github_token").into_response())
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
