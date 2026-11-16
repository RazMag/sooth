//! A unit on another machine: its page (`/machines/{id}/units/{file}`) and
//! start / stop / restart there. Everything goes through the machine's
//! agent; the agent itself refuses actions from a read-only hub.

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::fleet::{Listing, REACH_TIMEOUT};
use crate::machine::Action;
use crate::quadlet::UnitKind;
use crate::systemd::UnitStatus;
use crate::web::templates::remote_unit::{
    Freshness, LOG_LINES, RemoteUnitPage, blocked_for, live_block, menu_result, remote_unit_page,
};

/// Every kind, so any unit there can be found by its file name.
const ALL_KINDS: &[UnitKind] = &[
    UnitKind::Container,
    UnitKind::Pod,
    UnitKind::Volume,
    UnitKind::Network,
    UnitKind::Image,
    UnitKind::Build,
    UnitKind::Kube,
];

fn ago(at: u64) -> String {
    crate::web::templates::selfupdate::ago(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(at),
    )
}

pub async fn show(
    State(state): State<AppState>,
    session: Session,
    Path((id, file_name)): Path<(String, String)>,
) -> Result<Response, PageError> {
    let linked = state
        .fleet
        .get(&id)
        .ok_or_else(|| AppError::NotFound(format!("machine '{id}'")))?;
    let listing = state.fleet.listing(&linked, ALL_KINDS).await;
    let Some((units, _)) = listing.units() else {
        let reason = match &listing {
            Listing::Unreachable { reason } => reason.as_str(),
            _ => "no answer",
        };
        return Err(
            AppError::Remote(format!("{} isn't answering: {reason}", linked.entry.name)).into(),
        );
    };
    let (unit, status) = units
        .iter()
        .find(|(u, _)| u.file_name == file_name)
        .ok_or_else(|| AppError::NotFound(format!("{file_name} on {}", linked.entry.name)))?;
    let freshness = match &listing {
        Listing::Stale { as_of, reason, .. } => Freshness::Stale {
            ago: ago(*as_of),
            reason,
        },
        _ => Freshness::Live,
    };
    let logs = if listing.is_live() {
        match tokio::time::timeout(REACH_TIMEOUT, linked.machine().logs(&file_name, LOG_LINES))
            .await
        {
            Ok(Ok(lines)) => Ok(lines),
            Ok(Err(e)) => Err(e.user_message()),
            Err(_) => Err("no answer in time".to_string()),
        }
    } else {
        Err("the machine isn't answering".to_string())
    };
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    Ok(remote_unit_page(RemoteUnitPage {
        machine: &linked.entry,
        unit,
        status,
        freshness,
        logs: logs.as_deref().map_err(Clone::clone),
        csrf: &csrf,
        health: state.health.get(),
    })
    .into_response())
}

#[derive(Deserialize)]
pub struct ActForm {
    csrf_token: String,
}

/// `?style=menu`: answer for a list row's menu.
#[derive(Deserialize)]
pub struct ActQuery {
    style: Option<String>,
}

fn done(action: Action) -> &'static str {
    match action {
        Action::Start => "Started.",
        Action::Stop => "Stopped.",
        Action::Restart => "Restarted.",
    }
}

/// Runs the action and answers with what happened -- always 200, so htmx
/// swaps an error in too. Every answer also tells the page to redraw the
/// Services rows (`machine-units-changed`).
pub async fn act(
    State(state): State<AppState>,
    session: Session,
    Path((id, file_name, action)): Path<(String, String, String)>,
    Query(query): Query<ActQuery>,
    Form(form): Form<ActForm>,
) -> Result<Response, PageError> {
    if !crate::auth::csrf::verify(&session, &form.csrf_token).await {
        return Err(AppError::Csrf.into());
    }
    let action = Action::parse(&action).ok_or_else(|| AppError::NotFound(action.clone()))?;
    let linked = state
        .fleet
        .get(&id)
        .ok_or_else(|| AppError::NotFound(format!("machine '{id}'")))?;
    let machine = linked.machine();
    let outcome = machine.act(&file_name, action).await;
    let trigger = [("HX-Trigger", "machine-units-changed")];
    if query.style.as_deref() == Some("menu") {
        let message = outcome
            .as_ref()
            .map(|_| done(action))
            .map_err(AppError::user_message);
        let shown = match &message {
            Ok(done) => Ok(*done),
            Err(e) => Err(e.as_str()),
        };
        return Ok((trigger, menu_result(shown)).into_response());
    }
    let (status, message) = match outcome {
        Ok(status) => (status, Ok(done(action).to_string())),
        Err(e) => (
            machine
                .status(&file_name)
                .await
                .unwrap_or_else(|_| UnitStatus::not_found()),
            Err(e.user_message()),
        ),
    };
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let blocked = state
        .fleet
        .cached(&id, ALL_KINDS)
        .and_then(|(units, _)| units.into_iter().find(|(u, _)| u.file_name == file_name))
        .and_then(|(unit, _)| blocked_for(&linked.entry, &unit, true));
    Ok((
        trigger,
        live_block(
            &linked.entry,
            &file_name,
            &status,
            blocked,
            Some(message.as_deref().map_err(String::as_str)),
            &csrf,
        ),
    )
        .into_response())
}
