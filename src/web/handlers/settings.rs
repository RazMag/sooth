//! View and change persisted configuration. Writes go straight to the
//! resolved config TOML file and require a restart to take effect -- no
//! runtime-mutable `AppState`, no re-pointing the filesystem watcher or
//! D-Bus session on the fly. Simpler and safer for what's really a rarely-
//! touched settings screen.

use std::net::SocketAddr;
use std::path::Path;

use axum::Form;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::web::templates::settings::FormValues;
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
    let message = query
        .saved
        .is_some()
        .then_some("Saved. Restart sooth for changes to take effect.");
    templates::settings::page(
        &values,
        &state.config_path.display().to_string(),
        &csrf,
        message,
        None,
    )
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

    let entered_values = || FormValues {
        bind_addr: form.bind_addr.clone(),
        quadlet_dir: form.quadlet_dir.clone(),
        cookie_secure: form.cookie_secure.is_some(),
        log_filter: form.log_filter.clone(),
        session_idle_timeout_secs: form.session_idle_timeout_secs.clone(),
    };

    let Ok(bind_addr) = form.bind_addr.trim().parse::<SocketAddr>() else {
        let csrf = crate::auth::csrf::current(&session)
            .await
            .unwrap_or_default();
        let values = entered_values();
        return Ok((
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            templates::settings::page(
                &values,
                &state.config_path.display().to_string(),
                &csrf,
                None,
                Some("Bind address must be a valid host:port, e.g. 127.0.0.1:8420"),
            ),
        )
            .into_response());
    };
    let Ok(session_idle_timeout_secs) = form.session_idle_timeout_secs.trim().parse::<u64>() else {
        let csrf = crate::auth::csrf::current(&session)
            .await
            .unwrap_or_default();
        let values = entered_values();
        return Ok((
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            templates::settings::page(
                &values,
                &state.config_path.display().to_string(),
                &csrf,
                None,
                Some("Session idle timeout must be a whole number of seconds"),
            ),
        )
            .into_response());
    };
    let quadlet_dir = form.quadlet_dir.trim();
    if quadlet_dir.is_empty() {
        let csrf = crate::auth::csrf::current(&session)
            .await
            .unwrap_or_default();
        let values = entered_values();
        return Ok((
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            templates::settings::page(
                &values,
                &state.config_path.display().to_string(),
                &csrf,
                None,
                Some("Quadlet directory must not be empty"),
            ),
        )
            .into_response());
    }

    let toml_text = format!(
        "bind_addr = \"{bind_addr}\"\nquadlet_dir = \"{quadlet_dir}\"\ncookie_secure = {cookie_secure}\nlog_filter = \"{log_filter}\"\nsession_idle_timeout_secs = {session_idle_timeout_secs}\n",
        quadlet_dir = escape_toml_string(quadlet_dir),
        cookie_secure = form.cookie_secure.is_some(),
        log_filter = escape_toml_string(form.log_filter.trim()),
    );

    write_atomic(&state.config_path, &toml_text).map_err(|e| {
        AppError::Internal(anyhow::anyhow!(
            "failed to write {}: {e}",
            state.config_path.display()
        ))
    })?;

    tracing::info!(path = %state.config_path.display(), "settings saved");
    Ok(Redirect::to("/settings?saved=1").into_response())
}

fn escape_toml_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".sooth-settings-{}", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}
