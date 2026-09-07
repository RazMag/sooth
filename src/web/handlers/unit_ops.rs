//! Start/stop/restart/enable/disable -- exactly one implementation each,
//! mounted at every section's URL prefix (see `routes::mount_unit_routes`).
//! None of this differs by kind, so there's nothing to specialize.
//!
//! The status *badge* refreshes itself over SSE (`status-{service}`), but the
//! action controls -- which buttons to show -- depend on the same state and
//! can't be pushed from the SSE layer (it has no session, so no CSRF for the
//! forms). `actions` is the fragment the detail-page action row and the
//! list-row kebab re-fetch on that unit's `sse:status-{service}` event.

use axum::Form;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::FragmentError;
use crate::quadlet::autoupdate::AutoUpdateMode;
use crate::quadlet::discovery;
use crate::web::{core, templates};

#[derive(Deserialize)]
pub struct ActionForm {
    csrf_token: String,
}

macro_rules! action_handler {
    ($name:ident, $action:literal) => {
        pub async fn $name(
            State(state): State<AppState>,
            session: Session,
            Path(file_name): Path<String>,
            Form(form): Form<ActionForm>,
        ) -> Result<maud::Markup, FragmentError> {
            let outcome =
                core::execute_action(&state, &session, &file_name, &form.csrf_token, $action)
                    .await?;
            Ok(templates::status_badge(
                &outcome.unit.service_name(),
                &outcome.status,
            ))
        }
    };
}

action_handler!(start, "start");
action_handler!(stop, "stop");
action_handler!(restart, "restart");
action_handler!(enable, "enable");
action_handler!(disable, "disable");

#[derive(Deserialize)]
pub struct AutoUpdateForm {
    csrf_token: String,
    /// `"off"` (or anything unrecognised) clears the policy; `"registry"` /
    /// `"local"` set it.
    mode: String,
}

/// Sets the container's `[Container]` `AutoUpdate=` policy and returns the
/// freshly rendered control, which the `<select>` swaps in place -- the same
/// shape as `actions` returning `action_row`. Container-only; `core` rejects
/// other kinds.
pub async fn autoupdate(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
    Form(form): Form<AutoUpdateForm>,
) -> Result<maud::Markup, FragmentError> {
    let mode = AutoUpdateMode::parse(&form.mode);
    let unit = core::set_container_autoupdate(&state, &session, &form.csrf_token, &file_name, mode)
        .await?;
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    Ok(templates::autoupdate_control(&unit, &csrf))
}

/// A unit's verbatim `[Section]` config cards, re-rendered from disk. The
/// detail page wraps its Configuration column in an element that `hx-get`s
/// this on `sse:units-changed`, so an autostart toggle (which patches the
/// quadlet's `[Install]` section) or an external edit shows up without a
/// manual refresh -- the same way list rows track file changes.
pub async fn config(
    State(state): State<AppState>,
    Path(file_name): Path<String>,
) -> Result<maud::Markup, FragmentError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    Ok(templates::section_table(&unit))
}

#[derive(Deserialize)]
pub struct ActionsQuery {
    /// `?style=menu` -> just the kebab's status forms (for `.menu-actions`);
    /// anything else -> the full detail-page action row.
    style: Option<String>,
}

/// A unit's start/stop/restart/enable/disable controls, re-rendered against
/// its current status. The detail-page action row and each list-row kebab
/// wrap their status controls in an element that `hx-get`s this on the
/// unit's `sse:status-{service}` event, so the buttons track live state the
/// same way the badge does.
pub async fn actions(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
    Query(query): Query<ActionsQuery>,
) -> Result<maud::Markup, FragmentError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, &file_name)?;
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let menu = query.style.as_deref() == Some("menu");
    if unit.is_template() {
        // Templates have no action forms; the kebab slot just stays empty.
        return Ok(if menu {
            maud::html! {}
        } else {
            templates::banner(
                templates::BannerKind::Info,
                "Template unit — managed read-only. Use the CLI to instantiate it.",
            )
        });
    }
    let status = state.systemd.status(&unit.service_name()).await?;
    Ok(if menu {
        templates::kebab_action_forms(&unit, &status, &csrf)
    } else {
        templates::action_row(&unit, &status, &csrf)
    })
}
