//! Raw-textarea edit and delete -- exactly one implementation each, mounted
//! at every section's URL prefix. Edit stays raw for every kind, including
//! Containers and Pods (only *create* gets a structured form); delete is
//! identical everywhere. `.container` / `.build` units additionally get the
//! Name/Value env-var editor, backed by the same `env/<stem>.env` sidecar +
//! managed `EnvironmentFile=` line the create path writes.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::quadlet::{UnitKind, discovery, envfile, naming};
use crate::web::{core, templates};

fn wants_env(file_name: &str) -> bool {
    matches!(
        naming::kind_of(file_name),
        Some(UnitKind::Container | UnitKind::Build)
    )
}

/// Renders the edit form. `env_override` supplies the env-editor body on a
/// 422 redisplay (the text as submitted); otherwise it is loaded from the
/// sidecar for Container/Build units.
async fn render_edit(
    state: &AppState,
    session: &Session,
    file_name: &str,
    env_override: Option<&str>,
    error: Option<&str>,
) -> Result<Response, PageError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, file_name)?;
    let csrf = crate::auth::csrf::current(session)
        .await
        .unwrap_or_default();
    let host_vars = crate::hostenv::load().unwrap_or_default();
    let env_body = match env_override {
        Some(s) => s.to_string(),
        None if matches!(unit.kind, UnitKind::Container | UnitKind::Build) => {
            envfile::to_editor_lines(
                &envfile::load(&state.quadlet_dir, naming::stem(file_name)).unwrap_or_default(),
            )
        }
        None => String::new(),
    };
    let status = if error.is_some() {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        templates::generic::edit_unit_page(&unit, &csrf, &env_body, &host_vars, error),
    )
        .into_response())
}

pub async fn edit_form(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
) -> Result<Response, PageError> {
    render_edit(&state, &session, &file_name, None, None).await
}

#[derive(Deserialize)]
pub struct EditForm {
    csrf_token: String,
    contents: String,
    #[serde(default)]
    env_vars: Option<String>,
}

pub async fn edit_submit(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
    Form(form): Form<EditForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    let env_kind = match naming::kind_of(&file_name) {
        Some(k @ (UnitKind::Container | UnitKind::Build)) => Some(k),
        _ => None,
    };

    let Some(kind) = env_kind else {
        // Every other kind: the original kind-agnostic raw-edit path.
        return match core::edit_unit(
            &state,
            &session,
            &form.csrf_token,
            &file_name,
            &form.contents,
        )
        .await
        {
            Ok(unit) => Ok(Redirect::to(&core::unit_url(&unit)).into_response()),
            Err(AppError::Quadlet(e)) if e.is_client_error() => {
                render_edit(&state, &session, &file_name, None, Some(&e.to_string())).await
            }
            Err(e) => Err(e.into()),
        };
    };

    let stem = naming::stem(&file_name);
    let primary = kind.primary_section();
    let env_text = form.env_vars.as_deref().unwrap_or("");

    let pairs = match envfile::parse_editor_lines(env_text) {
        Ok(p) => p,
        Err(msg) => {
            return render_edit(&state, &session, &file_name, Some(env_text), Some(&msg)).await;
        }
    };

    let refval = envfile::reference_value(&state.quadlet_dir, stem);
    let contents =
        envfile::patch_environment_file(&form.contents, primary, &refval, !pairs.is_empty());

    // Remember the sidecar's prior state so a rejected write can be undone.
    let prev = envfile::load(&state.quadlet_dir, stem)
        .ok()
        .filter(|v| !v.is_empty());
    envfile::save(&state.quadlet_dir, stem, &pairs).map_err(|e| AppError::Internal(e.into()))?;

    match core::edit_unit(&state, &session, &form.csrf_token, &file_name, &contents).await {
        Ok(unit) => Ok(Redirect::to(&core::unit_url(&unit)).into_response()),
        Err(AppError::Quadlet(e)) if e.is_client_error() => {
            restore_sidecar(&state, stem, prev);
            render_edit(
                &state,
                &session,
                &file_name,
                Some(env_text),
                Some(&e.to_string()),
            )
            .await
        }
        Err(e) => {
            restore_sidecar(&state, stem, prev);
            Err(e.into())
        }
    }
}

fn restore_sidecar(state: &AppState, stem: &str, prev: Option<Vec<(String, String)>>) {
    let res = match prev {
        Some(vars) => envfile::save(&state.quadlet_dir, stem, &vars),
        None => envfile::delete(&state.quadlet_dir, stem),
    };
    if let Err(e) = res {
        tracing::warn!(stem, error = %e, "failed to roll back the env sidecar");
    }
}

#[derive(Deserialize)]
pub struct MoveForm {
    csrf_token: String,
    #[serde(default)]
    group: String,
}

/// Files the quadlet into a different group directory (blank `group` = the
/// quadlet-dir root). Kind-agnostic, mounted at every section prefix. The file
/// name -- and the URL -- is unchanged by a move. An htmx caller (the row
/// menu, drag-and-drop) gets `204` and lets the SSE `units-changed` refresh
/// redraw the table in place; a plain form post (the detail page) is
/// redirected back to the now-updated detail page.
pub async fn move_group(
    State(state): State<AppState>,
    session: Session,
    headers: axum::http::HeaderMap,
    Path(file_name): Path<String>,
    Form(form): Form<MoveForm>,
) -> Result<Response, PageError> {
    let unit = core::move_unit(&state, &session, &form.csrf_token, &file_name, &form.group).await?;
    if headers.contains_key("hx-request") {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Ok(Redirect::to(&core::unit_url(&unit)).into_response())
    }
}

#[derive(Deserialize)]
pub struct DeleteForm {
    csrf_token: String,
}

pub async fn delete(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
    Form(form): Form<DeleteForm>,
) -> Result<impl IntoResponse, PageError> {
    core::delete_unit(&state, &session, &form.csrf_token, &file_name).await?;
    if wants_env(&file_name)
        && let Err(e) = envfile::delete(&state.quadlet_dir, naming::stem(&file_name))
    {
        tracing::warn!(file = file_name, error = %e, "failed to remove the env sidecar");
    }
    let redirect = match naming::kind_of(&file_name) {
        Some(kind) => core::section_index_path(kind),
        None => "/units",
    };
    Ok(Redirect::to(redirect))
}
