//! Git Sync page: add/remove group-directory syncs and drive their
//! "Sync now"/"Force resync" actions. All mutation goes through
//! `AppState.git_sync` (`quadlet::gitsync::GitSyncManager`), which persists
//! to the config file and (unlike the rest of Settings) applies live -- no
//! restart needed.

use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, FragmentError, PageError};
use crate::quadlet::gitsync::{GitSyncConfig, GitSyncError, SyncStatus};
use crate::quadlet::{discovery, refs};
use crate::web::templates::gitsync::{AddFormValues, SyncEntry};
use crate::web::templates::settings::GithubTokenStatus;
use crate::web::templates::{self};

/// `204` for an htmx caller (relies on the `sse:git-sync-changed` row
/// refresh), a redirect back to the page for a plain form post -- same
/// convention as `handlers::groups::done`.
fn done(headers: &HeaderMap) -> Response {
    if headers.contains_key("hx-request") {
        StatusCode::NO_CONTENT.into_response()
    } else {
        Redirect::to("/git-sync").into_response()
    }
}

/// Every sync plus the podman secrets its group's units reference but that
/// don't exist yet -- an advisory badge only; it never affects `SyncState`
/// or blocks a sync. When podman can't be listed, reports nothing missing
/// rather than flagging every reference.
async fn entries(state: &AppState) -> Vec<SyncEntry> {
    let snapshot = state.git_sync.snapshot();
    let (Ok(existing), Ok(all)) = (
        crate::secrets::names().await,
        discovery::load_all(&state.quadlet_dir),
    ) else {
        return without_secrets(snapshot);
    };
    snapshot
        .into_iter()
        .map(|(config, status)| {
            let prefix = format!("{}/", config.group);
            let in_group = all
                .iter()
                .filter(|u| u.group == config.group || u.group.starts_with(&prefix));
            let missing = refs::missing_secrets(in_group, &existing);
            (config, status, missing)
        })
        .collect()
}

fn without_secrets(snapshot: Vec<(GitSyncConfig, SyncStatus)>) -> Vec<SyncEntry> {
    snapshot
        .into_iter()
        .map(|(c, s)| (c, s, Default::default()))
        .collect()
}

pub async fn page(State(state): State<AppState>, session: Session) -> impl IntoResponse {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let known_groups = discovery::list_groups(&state.quadlet_dir);
    templates::gitsync::page(
        &entries(&state).await,
        &csrf,
        &known_groups,
        &GithubTokenStatus::detect(&state.config, &state.config_path),
        state.health.get(),
    )
}

/// The status-table rows only -- re-fetched by the page on
/// `sse:git-sync-changed`, the git-sync analogue of `ports::rows`.
pub async fn rows(State(state): State<AppState>, session: Session) -> impl IntoResponse {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    templates::gitsync::rows(&entries(&state).await, &csrf)
}

#[derive(Deserialize)]
pub struct AddForm {
    csrf_token: String,
    group: String,
    remote: String,
    #[serde(default)]
    branch: String,
    poll_interval_secs: String,
}

pub async fn add(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<AddForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    let entered = AddFormValues {
        group: form.group.clone(),
        remote: form.remote.clone(),
        branch: form.branch.clone(),
        poll_interval_secs: form.poll_interval_secs.clone(),
    };
    let render_error = |msg: &str| {
        let csrf = &form.csrf_token;
        let known_groups = discovery::list_groups(&state.quadlet_dir);
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            templates::gitsync::page_with_add_error(
                &without_secrets(state.git_sync.snapshot()),
                csrf,
                &entered,
                &known_groups,
                &GithubTokenStatus::detect(&state.config, &state.config_path),
                msg,
                state.health.get(),
            ),
        )
            .into_response()
    };

    let Ok(poll_interval_secs) = form.poll_interval_secs.trim().parse::<u64>() else {
        return Ok(render_error(
            "Poll interval must be a whole number of seconds",
        ));
    };
    if poll_interval_secs == 0 {
        return Ok(render_error("Poll interval must be at least 1 second"));
    }
    let branch = form.branch.trim();

    match state
        .git_sync
        .add(
            form.group.trim().to_string(),
            form.remote.trim().to_string(),
            (!branch.is_empty()).then(|| branch.to_string()),
            poll_interval_secs,
            &state.quadlet_dir,
        )
        .await
    {
        Ok(()) => {
            tracing::info!(group = %form.group, "git-sync added via the web UI");
            Ok(Redirect::to("/git-sync").into_response())
        }
        Err(e) => Ok(render_error(&e.to_string())),
    }
}

// `group` travels as a form field, not a URL path segment: a nested group
// (`infra/monitoring`) contains `/`, which a `{group}` path segment can't
// carry -- the same reason `handlers::groups`'s rename/move/delete take it
// from the form body too.

#[derive(Deserialize)]
pub struct EditForm {
    csrf_token: String,
    group: String,
    remote: String,
    #[serde(default)]
    branch: String,
    poll_interval_secs: String,
}

/// Updates an existing sync's remote/branch/poll interval. htmx-only (the
/// card's "Edit" disclosure posts with `hx-swap="none"` and a
/// `hx-on::response-error` alert, same convention as `handlers::groups`'s
/// kebab menu) -- there's no plain-form fallback path here since the field
/// values already came from a live sync, not a fresh page load.
pub async fn edit(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<EditForm>,
) -> Result<Response, FragmentError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let poll_interval_secs = form
        .poll_interval_secs
        .trim()
        .parse::<u64>()
        .map_err(|_| GitSyncError::Validation("poll interval must be a whole number".into()))?;
    if poll_interval_secs == 0 {
        return Err(
            GitSyncError::Validation("poll interval must be at least 1 second".into()).into(),
        );
    }
    let branch = form.branch.trim();
    state
        .git_sync
        .edit(
            &form.group,
            form.remote.trim().to_string(),
            (!branch.is_empty()).then(|| branch.to_string()),
            poll_interval_secs,
            &state.quadlet_dir,
        )
        .await?;
    Ok(done(&headers))
}

#[derive(Deserialize)]
pub struct GroupForm {
    csrf_token: String,
    group: String,
}

pub async fn sync_now(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<GroupForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    state.git_sync.sync_now(&form.group)?;
    Ok(done(&headers))
}

pub async fn force_resync(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<GroupForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    state
        .git_sync
        .force_resync(&form.group, &state.quadlet_dir)
        .await?;
    Ok(done(&headers))
}

#[derive(Deserialize)]
pub struct DeleteForm {
    csrf_token: String,
    group: String,
    #[serde(default)]
    delete_files: Option<String>,
}

pub async fn delete(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<DeleteForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    state
        .git_sync
        .remove(&form.group, &state.quadlet_dir, form.delete_files.is_some())?;
    Ok(done(&headers))
}
