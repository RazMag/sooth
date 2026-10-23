//! The Registries page: a thin UI over podman's registry logins (see
//! `crate::registries`). A submitted password goes straight to
//! `podman login` on stdin and is never echoed back or logged; only the
//! registry and username are.

use axum::Form;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::html;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::registries;
use crate::web::templates;
use crate::web::templates::registries::RegistriesPage;

/// Session key for the one-shot notice after a successful POST -- a
/// registry like `host:5000/org` doesn't sit cleanly in a redirect's query
/// string, so it travels like the Secrets page's restart report.
const FLASH_KEY: &str = "registries.flash";

#[derive(Debug, Serialize, Deserialize)]
enum Flash {
    Saved {
        registry: String,
        cleared_session: bool,
    },
    Removed {
        registry: String,
    },
}

#[derive(Deserialize)]
pub struct PrefillQuery {
    /// From a row's "Update" button: prefills the login form.
    registry: Option<String>,
    username: Option<String>,
}

pub async fn index(
    State(state): State<AppState>,
    session: Session,
    Query(q): Query<PrefillQuery>,
) -> Result<Response, PageError> {
    let registry = q
        .registry
        .as_deref()
        .filter(|r| registries::valid_registry(r))
        .unwrap_or_default();
    let username = q
        .username
        .as_deref()
        .filter(|u| !u.chars().any(char::is_control))
        .unwrap_or_default();
    Ok(render(&state, &session, (registry, username), None, StatusCode::OK).await)
}

#[derive(Deserialize)]
pub struct LoginForm {
    csrf_token: String,
    registry: String,
    username: String,
    password: String,
}

pub async fn login(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let registry = registries::normalize_registry(&form.registry);
    let username = form.username.trim();

    match registries::login(registry, username, &form.password).await {
        Ok(outcome) => {
            // Registry and username only -- never the password.
            tracing::info!(
                registry,
                username,
                cleared_session = outcome.cleared_session,
                "registry login saved"
            );
            stash(
                &session,
                Flash::Saved {
                    registry: registry.to_string(),
                    cleared_session: outcome.cleared_session,
                },
            )
            .await;
            Ok(Redirect::to("/registries").into_response())
        }
        Err(e) if e.is_client_error() => Ok(render(
            &state,
            &session,
            (registry, username),
            Some(&e.to_string()),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await),
        Err(e) => Err(AppError::from(e).into()),
    }
}

#[derive(Deserialize)]
pub struct LogoutForm {
    csrf_token: String,
    registry: String,
}

pub async fn logout(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<LogoutForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let registry = form.registry.trim();
    match registries::logout(registry).await {
        Ok(()) => {
            tracing::info!(registry, "registry login removed");
            stash(
                &session,
                Flash::Removed {
                    registry: registry.to_string(),
                },
            )
            .await;
            Ok(Redirect::to("/registries").into_response())
        }
        Err(e) if e.is_client_error() => Ok(render(
            &state,
            &session,
            ("", ""),
            Some(&e.to_string()),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await),
        Err(e) => Err(AppError::from(e).into()),
    }
}

async fn stash(session: &Session, flash: Flash) {
    if let Err(e) = session.insert(FLASH_KEY, flash).await {
        tracing::warn!(error = %e, "could not store registries notice in session");
    }
}

async fn render(
    state: &AppState,
    session: &Session,
    (prefill_registry, prefill_username): (&str, &str),
    error: Option<&str>,
    status: StatusCode,
) -> Response {
    let csrf = crate::auth::csrf::current(session)
        .await
        .unwrap_or_default();
    let (logins, list_error) = match registries::list().await {
        Ok(l) => (l, None),
        Err(e) => {
            tracing::warn!(error = %e, "listing registry logins failed");
            (
                Vec::new(),
                Some(format!("Could not list registry logins: {e}")),
            )
        }
    };
    let saved_file = registries::auth_file()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    let notice = session
        .remove::<Flash>(FLASH_KEY)
        .await
        .ok()
        .flatten()
        .map(|flash| match flash {
            Flash::Saved {
                registry,
                cleared_session,
            } => html! {
                "Logged in to " code { (registry) } "."
                @if cleared_session {
                    " The older session-only login for it was removed, so this one is used."
                }
            },
            Flash::Removed { registry } => html! {
                "Login for " code { (registry) } " removed."
            },
        });

    (
        status,
        templates::registries::page(RegistriesPage {
            csrf: &csrf,
            logins: &logins,
            saved_file: &saved_file,
            prefill_registry,
            prefill_username,
            notice,
            error: error.or(list_error.as_deref()),
            health: state.health.get(),
        }),
    )
        .into_response()
}
