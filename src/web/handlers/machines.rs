//! The Machines page (`/machines`) and its Add machine flow: address and
//! code (`/machines/add`, which probes the agent), then the fingerprint
//! check and a name (`/machines/pair`). Rename and unpair are plain POSTs.

use axum::Form;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use maud::html;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::link::Fingerprint;
use crate::web::templates::machines::{
    ConfirmPage, MachinesPage, NavMachine, Row, confirm_page, machines_page, nav_machines,
};
use crate::web::templates::overview::MachineInfo;

const FLASH_KEY: &str = "machines.flash";

#[derive(Debug, Serialize, Deserialize)]
enum Flash {
    Paired { name: String, access: String },
    Unpaired { name: String, confirmed: bool },
    Renamed { name: String },
}

async fn stash(session: &Session, flash: Flash) {
    if let Err(e) = session.insert(FLASH_KEY, flash).await {
        tracing::warn!(error = %e, "could not store the machines notice in session");
    }
}

async fn csrf(session: &Session) -> String {
    crate::auth::csrf::current(session)
        .await
        .unwrap_or_default()
}

async fn check_csrf(session: &Session, token: &str) -> Result<(), PageError> {
    if crate::auth::csrf::verify(session, token).await {
        Ok(())
    } else {
        Err(AppError::Csrf.into())
    }
}

async fn reach_all(state: &AppState) -> Vec<Row> {
    state
        .fleet
        .check_all()
        .await
        .into_iter()
        .map(|(m, reach)| Row {
            via: m.via(),
            entry: m.entry,
            reach,
        })
        .collect()
}

async fn render_list(
    state: &AppState,
    session: &Session,
    address: &str,
    error: Option<&str>,
    status: StatusCode,
) -> Response {
    let csrf = csrf(session).await;
    let rows = reach_all(state).await;
    let notice = session
        .remove::<Flash>(FLASH_KEY)
        .await
        .ok()
        .flatten()
        .map(|flash| match flash {
            Flash::Paired { name, access } => html! {
                "Paired with " strong { (name) } " (" (access) " access)."
            },
            Flash::Unpaired { name, confirmed } => html! {
                "Unpaired " strong { (name) } "."
                @if !confirmed {
                    " It didn't answer, so it may still list this hub until you run "
                    code { "sooth --pair-code" } " there and pair again, or remove "
                    code { "link/hubs.toml" } " on it."
                }
            },
            Flash::Renamed { name } => html! { "Renamed to " strong { (name) } "." },
        });
    let hub_name = MachineInfo::detect().hostname;
    (
        status,
        machines_page(MachinesPage {
            csrf: &csrf,
            hub_name: &hub_name,
            hub_fingerprint: state.fleet.fingerprint().to_string(),
            rows: &rows,
            address,
            notice,
            error,
            ssh_key: state.fleet.ssh_setup().map(|s| s.key.public).ok(),
            health: state.health.get(),
        }),
    )
        .into_response()
}

/// The sidebar's other machines, each with a dot for how it last answered
/// (`fleet::monitor` keeps that current). Loaded by every page's sidebar and
/// redrawn on `machines-changed`.
pub async fn nav(State(state): State<AppState>) -> maud::Markup {
    let items: Vec<NavMachine> = state
        .fleet
        .machines()
        .into_iter()
        .map(|m| NavMachine {
            online: state.fleet.seen(&m.entry.id).map(|s| s.reach.is_online()),
            id: m.entry.id,
            name: m.entry.name,
        })
        .collect();
    nav_machines(&items)
}

pub async fn index(State(state): State<AppState>, session: Session) -> Response {
    render_list(&state, &session, "", None, StatusCode::OK).await
}

#[derive(Deserialize)]
pub struct AddForm {
    csrf_token: String,
    address: String,
    code: String,
    /// The agent's port behind an SSH address; blank for the default.
    #[serde(default)]
    agent_port: String,
}

/// The agent port field: blank is the default, anything else a port.
fn agent_port(field: &str) -> Result<Option<u16>, AppError> {
    let field = field.trim();
    if field.is_empty() {
        return Ok(None);
    }
    field
        .parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .map(Some)
        .ok_or_else(|| {
            crate::quadlet::QuadletError::Validation(format!("'{field}' isn't a port")).into()
        })
}

/// Step one: reach the agent without trusting it yet, and show its key.
pub async fn add(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<AddForm>,
) -> Result<Response, PageError> {
    check_csrf(&session, &form.csrf_token).await?;
    let address = form.address.trim();
    let probed = match agent_port(&form.agent_port) {
        Ok(port) => state.fleet.probe(address, port).await.map(|p| (p, port)),
        Err(e) => Err(e),
    };
    match probed {
        Ok(((fingerprint, hello), port)) => {
            let csrf = csrf(&session).await;
            Ok(confirm_page(ConfirmPage {
                csrf: &csrf,
                address,
                agent_port: port,
                code: form.code.trim(),
                fingerprint: fingerprint.to_string(),
                hello: &hello,
                name: &hello.hostname,
                error: None,
                health: state.health.get(),
            })
            .into_response())
        }
        Err(e) => Ok(render_list(
            &state,
            &session,
            address,
            Some(&e.user_message()),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await),
    }
}

#[derive(Deserialize)]
pub struct PairForm {
    csrf_token: String,
    address: String,
    code: String,
    fingerprint: String,
    name: String,
    #[serde(default)]
    agent_port: String,
}

/// Step two: pair over a connection pinned to the key the person confirmed.
pub async fn pair(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<PairForm>,
) -> Result<Response, PageError> {
    check_csrf(&session, &form.csrf_token).await?;
    let fingerprint = Fingerprint::parse(&form.fingerprint)
        .ok_or_else(|| AppError::NotFound("fingerprint".into()))?;
    let added = match agent_port(&form.agent_port) {
        Ok(port) => {
            state
                .fleet
                .add(&form.address, port, fingerprint, &form.code, &form.name)
                .await
        }
        Err(e) => Err(e),
    };
    match added {
        Ok(entry) => {
            stash(
                &session,
                Flash::Paired {
                    name: entry.name,
                    access: entry.permission.label().to_string(),
                },
            )
            .await;
            Ok(Redirect::to("/machines").into_response())
        }
        // Back to step one: a wrong or expired code needs a new one typed in.
        Err(e) => Ok(render_list(
            &state,
            &session,
            &form.address,
            Some(&e.user_message()),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await),
    }
}

#[derive(Deserialize)]
pub struct RenameForm {
    csrf_token: String,
    id: String,
    name: String,
}

pub async fn rename(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<RenameForm>,
) -> Result<Response, PageError> {
    check_csrf(&session, &form.csrf_token).await?;
    match state.fleet.rename(&form.id, &form.name) {
        Ok(()) => {
            stash(
                &session,
                Flash::Renamed {
                    name: form.name.trim().to_string(),
                },
            )
            .await;
            Ok(Redirect::to("/machines").into_response())
        }
        Err(e) if e.status() == StatusCode::UNPROCESSABLE_ENTITY => Ok(render_list(
            &state,
            &session,
            "",
            Some(&e.user_message()),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await),
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize)]
pub struct RemoveForm {
    csrf_token: String,
    id: String,
}

pub async fn remove(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<RemoveForm>,
) -> Result<Response, PageError> {
    check_csrf(&session, &form.csrf_token).await?;
    let name = state
        .fleet
        .get(&form.id)
        .map(|m| m.entry.name)
        .unwrap_or_else(|| form.id.clone());
    let confirmed = state.fleet.remove(&form.id).await?;
    stash(&session, Flash::Unpaired { name, confirmed }).await;
    Ok(Redirect::to("/machines").into_response())
}
