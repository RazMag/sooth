//! The one create implementation for every section. Nobody hand-builds a
//! quadlet field-by-field -- they write the INI -- so every "New" page is a
//! file name plus a live-validated code editor (see `handlers::validate`
//! and `static/app.js`), pre-filled with a starter skeleton for whichever
//! section it was reached from. `create` itself doesn't need to know which
//! entry point it was reached from: the created unit's kind comes from the
//! file extension actually typed, and the post-create redirect is built
//! from that kind via `core::section_path`.

use axum::Form;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::web::templates::NavItem;
use crate::web::{core, templates};

async fn new_form(
    session: Session,
    active: Option<NavItem>,
    action: &str,
    skeleton: &str,
) -> maud::Markup {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    templates::generic::new_unit_page(&csrf, active, action, skeleton, None)
}

pub async fn containers_new_form(
    session: Session,
    Query(query): Query<PodQuery>,
) -> impl IntoResponse {
    let skeleton = match query.pod {
        Some(pod) => format!("[Container]\nImage=\nPod={pod}\n"),
        None => "[Container]\nImage=\n".to_string(),
    };
    new_form(session, Some(NavItem::Services), "/containers", &skeleton).await
}

#[derive(Deserialize)]
pub struct PodQuery {
    pod: Option<String>,
}

pub async fn pods_new_form(session: Session) -> impl IntoResponse {
    new_form(session, Some(NavItem::Services), "/pods", "[Pod]\n").await
}
pub async fn volumes_new_form(session: Session) -> impl IntoResponse {
    new_form(session, Some(NavItem::Volumes), "/volumes", "[Volume]\n").await
}
pub async fn networks_new_form(session: Session) -> impl IntoResponse {
    new_form(session, Some(NavItem::Networks), "/networks", "[Network]\n").await
}
pub async fn images_new_form(session: Session) -> impl IntoResponse {
    new_form(
        session,
        Some(NavItem::Images),
        "/images",
        "[Image]\nImage=\n",
    )
    .await
}
pub async fn units_new_form(session: Session) -> impl IntoResponse {
    new_form(session, None, "/units", "").await
}

/// Which sidebar item and POST target to redisplay a rejected "New" form
/// with -- derived from the file name's extension, since a validation
/// failure means we don't have a successfully parsed unit to derive it
/// from otherwise.
fn redisplay_target(file_name: &str) -> (Option<NavItem>, &'static str) {
    use crate::quadlet::{UnitKind, naming};
    match naming::kind_of(file_name) {
        Some(UnitKind::Container) => (Some(NavItem::Services), "/containers"),
        Some(UnitKind::Pod) => (Some(NavItem::Services), "/pods"),
        Some(UnitKind::Volume) => (Some(NavItem::Volumes), "/volumes"),
        Some(UnitKind::Network) => (Some(NavItem::Networks), "/networks"),
        Some(UnitKind::Image | UnitKind::Build) => (Some(NavItem::Images), "/images"),
        _ => (None, "/units"),
    }
}

#[derive(Deserialize)]
pub struct CreateForm {
    csrf_token: String,
    file_name: String,
    contents: String,
}

pub async fn create(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CreateForm>,
) -> Result<Response, PageError> {
    match core::create_unit(
        &state,
        &session,
        &form.csrf_token,
        &form.file_name,
        &form.contents,
    )
    .await
    {
        Ok(unit) => Ok(Redirect::to(&core::unit_url(&unit)).into_response()),
        Err(AppError::Quadlet(e)) if e.is_client_error() => {
            tracing::warn!(file = form.file_name, error = %e, "rejected new quadlet file");
            let csrf = crate::auth::csrf::current(&session)
                .await
                .unwrap_or_default();
            let (active, action) = redisplay_target(&form.file_name);
            Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                templates::generic::new_unit_page(
                    &csrf,
                    active,
                    action,
                    &form.contents,
                    Some(&e.to_string()),
                ),
            )
                .into_response())
        }
        Err(e) => Err(e.into()),
    }
}
