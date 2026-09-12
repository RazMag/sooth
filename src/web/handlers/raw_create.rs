//! The one create implementation for every section. Nobody hand-builds a
//! quadlet field-by-field -- they write the INI -- so every "New" page is a
//! file-name stem plus a live-validated code editor (see `handlers::validate`
//! and `static/app.js`), pre-filled with a starter skeleton for whichever
//! section it was reached from. The section fixes the unit's *kind*: its
//! extension is applied server-side (`naming::compose_file_name`), so the
//! stem field never needs `.container` typed into it. `/units/new` offers a
//! `<select>` of every kind instead.
//!
//! For `.container` / `.build` the form also carries a Name/Value environment
//! editor; on submit sooth writes `<quadlet_dir>/env/<stem>.env` and wires a
//! managed `EnvironmentFile=` line into the unit's primary section.

use axum::Form;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::quadlet::{UnitKind, discovery, envfile, naming};
use crate::web::templates::NavItem;
use crate::web::templates::generic::{KindChoice, NewUnitPage};
use crate::web::{core, templates};

async fn new_form(
    state: &AppState,
    session: Session,
    active: Option<NavItem>,
    action: &str,
    kind: KindChoice,
    skeleton: &str,
) -> maud::Markup {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let host_vars = crate::hostenv::load().unwrap_or_default();
    let known_groups = discovery::list_groups(&state.quadlet_dir);
    templates::generic::new_unit_page(NewUnitPage {
        csrf: &csrf,
        active,
        action,
        kind,
        editor_body: skeleton,
        stem_prefill: "",
        group_prefill: "",
        known_groups: &known_groups,
        env_vars_body: "",
        host_vars: &host_vars,
        error: None,
        health: state.health.get(),
    })
}

pub async fn containers_new_form(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<PodQuery>,
) -> impl IntoResponse {
    let skeleton = match query.pod {
        Some(pod) => format!("[Container]\nImage=\nPod={pod}\n"),
        None => "[Container]\nImage=\n".to_string(),
    };
    new_form(
        &state,
        session,
        Some(NavItem::Services),
        "/containers",
        KindChoice::Fixed(UnitKind::Container),
        &skeleton,
    )
    .await
}

#[derive(Deserialize)]
pub struct PodQuery {
    pod: Option<String>,
}

pub async fn pods_new_form(State(state): State<AppState>, session: Session) -> impl IntoResponse {
    new_form(
        &state,
        session,
        Some(NavItem::Services),
        "/pods",
        KindChoice::Fixed(UnitKind::Pod),
        "[Pod]\n",
    )
    .await
}
pub async fn volumes_new_form(
    State(state): State<AppState>,
    session: Session,
) -> impl IntoResponse {
    new_form(
        &state,
        session,
        Some(NavItem::Volumes),
        "/volumes",
        KindChoice::Fixed(UnitKind::Volume),
        "[Volume]\n",
    )
    .await
}
pub async fn networks_new_form(
    State(state): State<AppState>,
    session: Session,
) -> impl IntoResponse {
    new_form(
        &state,
        session,
        Some(NavItem::Networks),
        "/networks",
        KindChoice::Fixed(UnitKind::Network),
        "[Network]\n",
    )
    .await
}
pub async fn images_new_form(State(state): State<AppState>, session: Session) -> impl IntoResponse {
    new_form(
        &state,
        session,
        Some(NavItem::Images),
        "/images",
        KindChoice::Fixed(UnitKind::Image),
        "[Image]\nImage=\n",
    )
    .await
}
pub async fn units_new_form(State(state): State<AppState>, session: Session) -> impl IntoResponse {
    new_form(
        &state,
        session,
        None,
        "/units",
        KindChoice::Choose {
            selected: UnitKind::Container,
        },
        "",
    )
    .await
}

/// Which sidebar item, POST target, and file-name-field shape to redisplay a
/// rejected "New" form with. `origin` (a hidden field on the form) says which
/// entry point it came from -- a `<select>`-driven `/units/new` form must not
/// collapse to a fixed suffix, and vice versa.
fn redisplay_target(
    origin: Option<&str>,
    kind: UnitKind,
) -> (Option<NavItem>, &'static str, KindChoice) {
    match origin {
        Some("units") => (None, "/units", KindChoice::Choose { selected: kind }),
        _ => (
            NavItem::for_kind(kind),
            core::section_path(kind),
            KindChoice::Fixed(kind),
        ),
    }
}

#[derive(Deserialize)]
pub struct CreateForm {
    csrf_token: String,
    /// The file-name *stem* only -- the extension comes from `kind`.
    file_name: String,
    /// The unit kind's extension (`container` .. `image`), from the hidden
    /// input or the `<select>`.
    kind: String,
    /// Optional group (subdirectory) to file the new unit under; blank = root.
    #[serde(default)]
    group: Option<String>,
    /// `section` | `units` -- which "New" form shape to redisplay on a 422.
    #[serde(default)]
    origin: Option<String>,
    contents: String,
    /// `KEY=VALUE` lines from the env editor; only used for Container/Build.
    #[serde(default)]
    env_vars: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<CreateForm>,
) -> Result<Response, PageError> {
    // Verified here as well as in `core::create_unit`: we now touch the disk
    // (the env sidecar) before that call, so the CSRF gate has to come first.
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    let stem = form.file_name.trim().to_string();
    let group = form.group.as_deref().unwrap_or("").trim().to_string();
    let origin = form.origin.as_deref();
    let env_text = form.env_vars.as_deref().unwrap_or("");

    let Some(kind) = UnitKind::from_extension(form.kind.trim()) else {
        return Ok(reject_new(
            &state,
            &session,
            origin,
            RejectCtx {
                kind: UnitKind::Container,
                stem: &stem,
                group: &group,
            },
            &form.contents,
            env_text,
            "Pick a unit type.",
        )
        .await);
    };

    let file_name = match naming::compose_file_name(&stem, kind) {
        Ok(f) => f,
        Err(e) => {
            return Ok(reject_new(
                &state,
                &session,
                origin,
                RejectCtx {
                    kind,
                    stem: &stem,
                    group: &group,
                },
                &form.contents,
                env_text,
                &e.to_string(),
            )
            .await);
        }
    };

    if !naming::valid_group(&group) {
        return Ok(reject_new(
            &state,
            &session,
            origin,
            RejectCtx {
                kind,
                stem: &stem,
                group: &group,
            },
            &form.contents,
            env_text,
            &format!("invalid group '{group}'"),
        )
        .await);
    }

    let wants_env = matches!(kind, UnitKind::Container | UnitKind::Build);

    let pairs = if wants_env {
        match envfile::parse_editor_lines(env_text) {
            Ok(p) => p,
            Err(msg) => {
                return Ok(reject_new(
                    &state,
                    &session,
                    origin,
                    RejectCtx {
                        kind,
                        stem: &stem,
                        group: &group,
                    },
                    &form.contents,
                    env_text,
                    &msg,
                )
                .await);
            }
        }
    } else {
        Vec::new()
    };

    let contents = if wants_env {
        let refval = envfile::reference_value(&state.quadlet_dir, &stem);
        envfile::patch_environment_file(
            &form.contents,
            kind.primary_section(),
            &refval,
            !pairs.is_empty(),
        )
    } else {
        form.contents.clone()
    };

    // Sidecar first: `writer::validate` runs the podman generator dry-run,
    // which could flag a unit whose `EnvironmentFile=` target doesn't exist.
    let sidecar_existed = envfile::path_for(&state.quadlet_dir, &stem).exists();
    if wants_env {
        envfile::save(&state.quadlet_dir, &stem, &pairs)
            .map_err(|e| AppError::Internal(e.into()))?;
    }

    match core::create_unit(
        &state,
        &session,
        &form.csrf_token,
        &file_name,
        &group,
        &contents,
    )
    .await
    {
        Ok(unit) => Ok(Redirect::to(&core::unit_url(&unit)).into_response()),
        Err(AppError::Quadlet(e)) if e.is_client_error() => {
            tracing::warn!(file = file_name, error = %e, "rejected new quadlet file");
            if wants_env && !sidecar_existed {
                let _ = envfile::delete(&state.quadlet_dir, &stem);
            }
            Ok(reject_new(
                &state,
                &session,
                origin,
                RejectCtx {
                    kind,
                    stem: &stem,
                    group: &group,
                },
                &form.contents,
                env_text,
                &e.to_string(),
            )
            .await)
        }
        Err(e) => {
            if wants_env && !sidecar_existed {
                let _ = envfile::delete(&state.quadlet_dir, &stem);
            }
            Err(e.into())
        }
    }
}

/// The originating "New" form's identity, carried through a 422 redisplay so
/// the stem, group, and kind picker come back filled in as submitted.
struct RejectCtx<'a> {
    kind: UnitKind,
    stem: &'a str,
    group: &'a str,
}

/// Renders the "New" form again with a 422, preserving the stem, group, the
/// editor body as typed, and the env-var text.
async fn reject_new(
    state: &AppState,
    session: &Session,
    origin: Option<&str>,
    ctx: RejectCtx<'_>,
    editor_body: &str,
    env_vars_body: &str,
    error: &str,
) -> Response {
    let csrf = crate::auth::csrf::current(session)
        .await
        .unwrap_or_default();
    let (active, action, choice) = redisplay_target(origin, ctx.kind);
    let host_vars = crate::hostenv::load().unwrap_or_default();
    let known_groups = discovery::list_groups(&state.quadlet_dir);
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        templates::generic::new_unit_page(NewUnitPage {
            csrf: &csrf,
            active,
            action,
            kind: choice,
            editor_body,
            stem_prefill: ctx.stem,
            group_prefill: ctx.group,
            known_groups: &known_groups,
            env_vars_body,
            host_vars: &host_vars,
            error: Some(error),
            health: state.health.get(),
        }),
    )
        .into_response()
}
