//! Group directories. A "group" is just a subdirectory of the quadlet dir
//! that units are filed under; podman recurses into it and the name has no
//! effect on the generated unit. These handlers create an *empty* group and
//! re-parent an existing one -- every other group operation is a side effect
//! of moving/creating/deleting a unit (`core::move_unit`, `core::create_unit`,
//! `writer::delete`).

use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::events::DashboardEvent;
use crate::quadlet::{QuadletError, naming};
use crate::web::core;

/// `204` for an htmx caller (which uses `hx-swap="none"` and lets the SSE
/// `units-changed` refresh redraw the table), a redirect to the home page for
/// a plain form post.
fn done(headers: &HeaderMap) -> Response {
    if headers.contains_key("hx-request") {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Redirect::to("/").into_response()
    }
}

#[derive(Deserialize)]
pub struct CreateGroupForm {
    csrf_token: String,
    #[serde(default)]
    group: String,
    /// When set (the "add subgroup" control on a group row), `group` is the
    /// child segment and this is the parent path it nests under.
    #[serde(default)]
    parent: Option<String>,
}

/// `POST /groups` -- `mkdir -p <quadlet_dir>/<group>` (optionally under
/// `parent`). Broadcasts `UnitsChanged` so every open list re-renders and
/// shows the new (empty) section.
pub async fn create(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<CreateGroupForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let child = form.group.trim().trim_matches('/');
    let group = match form.parent.as_deref().map(|p| p.trim().trim_matches('/')) {
        Some(p) if !p.is_empty() => format!("{p}/{child}"),
        _ => child.to_string(),
    };
    if group.is_empty() || !naming::valid_group(&group) {
        return Err(QuadletError::Validation(format!("invalid group '{group}'")).into());
    }
    std::fs::create_dir_all(state.quadlet_dir.join(&group))
        .map_err(|e| AppError::Internal(e.into()))?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(group, "group directory created");
    Ok(done(&headers))
}

#[derive(Deserialize)]
pub struct MoveGroupForm {
    csrf_token: String,
    /// The group being moved.
    group: String,
    /// Its new parent path; blank = the quadlet-dir root.
    #[serde(default)]
    parent: String,
}

/// `POST /groups/move` -- re-parent a whole group directory (drag-and-drop in
/// the table). Delegates the traversal/collision checks to
/// `core::move_group_dir`.
pub async fn move_group(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<MoveGroupForm>,
) -> Result<Response, PageError> {
    core::move_group_dir(
        &state,
        &session,
        &form.csrf_token,
        &form.group,
        &form.parent,
    )
    .await?;
    Ok(done(&headers))
}

#[derive(Deserialize)]
pub struct RenameGroupForm {
    csrf_token: String,
    group: String,
    #[serde(default)]
    name: String,
}

/// `POST /groups/rename` -- change a group's last path segment in place.
pub async fn rename(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<RenameGroupForm>,
) -> Result<Response, PageError> {
    core::rename_group(&state, &session, &form.csrf_token, &form.group, &form.name).await?;
    Ok(done(&headers))
}

#[derive(Deserialize)]
pub struct DeleteGroupForm {
    csrf_token: String,
    group: String,
}

/// `POST /groups/delete` -- remove an empty group directory tree.
pub async fn delete(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<DeleteGroupForm>,
) -> Result<Response, PageError> {
    core::delete_group(&state, &session, &form.csrf_token, &form.group).await?;
    Ok(done(&headers))
}
