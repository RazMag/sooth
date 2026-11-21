//! The Services page: every machine's containers and pods in one table, a
//! header row per machine, narrowed to one with `?m=` (see [`Scope`]).
//! Another machine's units are drawn from what its agent answers now, or
//! what it last answered when it doesn't (see `fleet::Listing`).

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use futures_util::future::join_all;
use maud::{Markup, html};
use tower_sessions::Session;

use crate::config::AppState;
use crate::containerhealth::ContainerHealth;
use crate::error::{AppError, FragmentError, PageError};
use crate::fleet::{Linked, Listing};
use crate::link::trust::Permission;
use crate::machine::Action;
use crate::quadlet::ports::SplitPorts;
use crate::quadlet::refs::MissingRefs;
use crate::quadlet::{QuadletUnit, UnitKind, discovery};
use crate::systemd::UnitStatus;
use crate::web::core::{self, NewerTags};
use crate::web::handlers::list::resolve_list_view;
use crate::web::scope::{LOCAL, Scope};
use crate::web::templates::list::{
    LOCAL_REFRESH, ListContext, ListView, OnMachine, TableBody, list_rows,
};
use crate::web::templates::overview::MachineInfo;
use crate::web::templates::services::{
    HeaderState, MachineHeader, ServicesPage, Stats, machine_row,
};
use crate::web::templates::{self};

const KINDS: &[UnitKind] = templates::services::SPEC.kinds;

pub fn compute_stats(units: &[(QuadletUnit, UnitStatus)]) -> Stats {
    let running = units.iter().filter(|(_, s)| s.is_active()).count();
    let failed = units.iter().filter(|(_, s)| s.is_failed()).count();
    Stats {
        total: units.len(),
        running,
        failed,
    }
}

/// Every listed pod's published ports, split onto the members serving them.
async fn split_ports(
    units: &[(QuadletUnit, UnitStatus)],
    all: &[QuadletUnit],
) -> HashMap<String, SplitPorts> {
    let pods: Vec<&QuadletUnit> = units
        .iter()
        .map(|(u, _)| u)
        .filter(|u| u.kind == UnitKind::Pod)
        .collect();
    core::split_pod_ports(&pods, all).await
}

/// `?view=` (the layout switch) and `?m=` (which machine, see
/// [`Scope`]).
#[derive(serde::Deserialize)]
pub struct ServicesQuery {
    pub view: Option<String>,
    pub m: Option<String>,
}

/// One machine's share of the table.
enum Block {
    /// This hub's own units, with everything it knows about them locally.
    Local {
        units: Vec<(QuadletUnit, UnitStatus)>,
        all: Vec<QuadletUnit>,
        known: Vec<String>,
        synced: Vec<String>,
        missing: HashMap<String, MissingRefs>,
        tags: HashMap<String, NewerTags>,
        ports: HashMap<String, SplitPorts>,
        health: HashMap<String, ContainerHealth>,
    },
    Remote {
        linked: Linked,
        listing: Listing,
    },
}

/// The units in scope, a block per machine, fetched side by side.
async fn load_blocks(state: &AppState, scope: &Scope) -> Result<Vec<Block>, AppError> {
    let remotes = scope.remotes(&state.fleet);
    let remote = join_all(remotes.into_iter().map(|linked| async move {
        let listing = state.fleet.listing(&linked, KINDS).await;
        Block::Remote { linked, listing }
    }));
    let local = async {
        if !scope.includes_local() {
            return Ok::<_, AppError>(None);
        }
        let (units, all) = core::load_units_and_siblings(state, KINDS).await?;
        let missing = core::missing_refs(state, &units).await;
        let tags = core::newer_tags(state, &units, &all);
        let ports = split_ports(&units, &all).await;
        let health = crate::containerhealth::health_of(units.iter().map(|(u, _)| u)).await;
        Ok(Some(Block::Local {
            known: discovery::list_groups(&state.quadlet_dir),
            synced: state.git_sync.synced_groups(),
            units,
            all,
            missing,
            tags,
            ports,
            health,
        }))
    };
    let (local, remote) = tokio::join!(local, remote);
    Ok(local?.into_iter().chain(remote).collect())
}

/// The table's rows: a header row per machine when there's more than this
/// hub's own (or the one in scope is another machine), then its units.
fn render_rows(blocks: &[Block], csrf: &str, view: ListView, scope: &Scope) -> Markup {
    let headed = blocks.len() > 1 || blocks.iter().any(|b| matches!(b, Block::Remote { .. }));
    let spec = &templates::services::SPEC;
    let none_missing = HashMap::new();
    let none_tags = HashMap::new();
    let hub_name = MachineInfo::detect().hostname;
    let retry = scope.link("/services/rows");
    html! {
        @for block in blocks {
            @match block {
                Block::Local { units, all, known, synced, missing, tags, ports, health } => {
                    @if headed {
                        (machine_row(&MachineHeader {
                            id: LOCAL,
                            name: &hub_name,
                            local: true,
                            state: HeaderState::Live { count: listed(units) },
                        }, spec.width(), &retry))
                    }
                    (list_rows(spec, units, csrf, all, &ListContext {
                        known,
                        synced,
                        missing,
                        tags,
                        ports: Some(ports),
                        health: Some(health),
                        view,
                        on: None,
                    }))
                }
                Block::Remote { linked, listing } => {
                    @let entry = &linked.entry;
                    @let state = match listing {
                        Listing::Live((units, _)) => HeaderState::Live { count: listed(units) },
                        Listing::Stale { listing: (units, _), as_of, reason } => HeaderState::Stale {
                            count: listed(units),
                            ago: ago_secs(*as_of),
                            reason,
                        },
                        Listing::Unreachable { reason } => HeaderState::Unreachable { reason },
                    };
                    (machine_row(&MachineHeader { id: &entry.id, name: &entry.name, local: false, state }, spec.width(), &retry))
                    @if let Some((units, all)) = listing.units() {
                        @let on = OnMachine {
                            id: &entry.id,
                            actions: listing.is_live() && entry.permission == Permission::Full,
                        };
                        (list_rows(spec, units, csrf, all, &ListContext {
                            known: &[],
                            synced: &[],
                            missing: &none_missing,
                            tags: &none_tags,
                            ports: None,
                            health: None,
                            view,
                            on: Some(&on),
                        }))
                    }
                }
            }
        }
    }
}

/// How many rows the Services table lists of `units` (pods loaded only to
/// head their members don't count -- here every pod is listed anyway).
fn listed(units: &[(QuadletUnit, UnitStatus)]) -> usize {
    units
        .iter()
        .filter(|(u, _)| KINDS.contains(&u.kind))
        .count()
}

fn ago_secs(at: u64) -> String {
    templates::selfupdate::ago(std::time::UNIX_EPOCH + std::time::Duration::from_secs(at))
}

fn stats_of(blocks: &[Block]) -> Stats {
    let units: Vec<(QuadletUnit, UnitStatus)> = blocks
        .iter()
        .flat_map(|b| match b {
            Block::Local { units, .. } => units.clone(),
            Block::Remote { listing, .. } => {
                listing.units().map(|(u, _)| u.clone()).unwrap_or_default()
            }
        })
        .collect();
    compute_stats(&units)
}

/// What redraws the rows: this hub's own changes, a machine coming or
/// going, an action on another machine, and -- since other machines don't
/// push their changes yet -- every 30 seconds while any is in scope.
fn refresh_trigger(scope: &Scope, fleet: &crate::fleet::Fleet) -> &'static str {
    if scope.remotes(fleet).is_empty() {
        LOCAL_REFRESH
    } else {
        "sse:units-changed, sse:image-tags-changed, sse:machines-changed, machine-units-changed from:body, every 30s"
    }
}

pub async fn page(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<ServicesQuery>,
) -> Result<impl IntoResponse, PageError> {
    let scope = Scope::resolve(&state.fleet, query.m.as_deref())?;
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let blocks = load_blocks(&state, &scope).await?;
    let view = resolve_list_view(&session, query.view.as_deref()).await;
    let (known, synced) = match blocks.first() {
        Some(Block::Local { known, synced, .. }) => (known.clone(), synced.clone()),
        _ => (Vec::new(), Vec::new()),
    };
    let none_missing = HashMap::new();
    let none_tags = HashMap::new();
    let rows_route = scope.link("/services/rows");
    let counts_route = scope.link("/services/counts");
    Ok(templates::services::services_page(&ServicesPage {
        stats: &stats_of(&blocks),
        csrf: &csrf,
        context: &ListContext {
            known: &known,
            synced: &synced,
            missing: &none_missing,
            tags: &none_tags,
            ports: None,
            health: None,
            view,
            on: None,
        },
        body: &TableBody {
            rows: render_rows(&blocks, &csrf, view, &scope),
            route: &rows_route,
            refresh: refresh_trigger(&scope, &state.fleet),
        },
        counts_route: &counts_route,
        scopes: scope.choices(&state.fleet, "/services"),
        creates: scope.includes_local(),
        health: state.health.get(),
    }))
}

pub async fn rows(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<ServicesQuery>,
) -> Result<impl IntoResponse, FragmentError> {
    let scope = Scope::resolve(&state.fleet, query.m.as_deref())?;
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let blocks = load_blocks(&state, &scope).await?;
    let view = resolve_list_view(&session, None).await;
    Ok(render_rows(&blocks, &csrf, view, &scope))
}

/// The stats bar. Other machines' units count as they were last listed, so
/// a status change here doesn't make the hub ask every machine again.
pub async fn counts(
    State(state): State<AppState>,
    Query(query): Query<ServicesQuery>,
) -> Result<impl IntoResponse, FragmentError> {
    let scope = Scope::resolve(&state.fleet, query.m.as_deref())?;
    let mut units = if scope.includes_local() {
        core::load_units_for_kinds(&state, KINDS).await?
    } else {
        Vec::new()
    };
    for m in scope.remotes(&state.fleet) {
        if let Some((remote, _)) = state.fleet.cached(&m.entry.id, KINDS) {
            units.extend(remote);
        }
    }
    Ok(templates::services::counts_fragment(&compute_stats(&units)))
}

#[derive(serde::Deserialize)]
pub struct BulkForm {
    csrf_token: String,
    /// `start`, `stop` or `restart` -- the bulk bar button pressed.
    action: String,
    /// The selected units' file names, comma-separated (filled in by
    /// `frontend/bulk.js`).
    units: String,
}

/// The bulk bar: one action on every selected unit, in turn. Answers with
/// a one-line result for the bar (always 200 -- htmx drops other bodies);
/// the rows' badges update themselves over SSE.
pub async fn bulk(
    State(state): State<AppState>,
    session: Session,
    axum::Form(form): axum::Form<BulkForm>,
) -> Response {
    let (action, done): (&'static str, &str) = match form.action.as_str() {
        "start" => ("start", "Started"),
        "stop" => ("stop", "Stopped"),
        "restart" => ("restart", "Restarted"),
        _ => {
            return templates::services::bulk_result(
                0,
                &[("".into(), "unknown action".into())],
                "",
            )
            .into_response();
        }
    };
    let mut ok = 0;
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut remote_acted = false;
    for name in form
        .units
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        // Another machine's unit comes as `machine/file` (see
        // `ListContext::key`); a file name never holds a `/`.
        let result = match name.split_once('/') {
            Some((machine, file)) => {
                remote_acted = true;
                act_remote(&state, &session, &form.csrf_token, machine, file, action).await
            }
            None => core::execute_action(&state, &session, name, &form.csrf_token, action)
                .await
                .map(|_| ())
                .map_err(|e| e.0),
        };
        match result {
            Ok(()) => ok += 1,
            Err(e) => failed.push((name.to_string(), e.to_string())),
        }
    }
    let result = templates::services::bulk_result(ok, &failed, done);
    if remote_acted {
        ([("HX-Trigger", "machine-units-changed")], result).into_response()
    } else {
        result.into_response()
    }
}

async fn act_remote(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    machine: &str,
    file: &str,
    action: &str,
) -> Result<(), AppError> {
    if !crate::auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    let linked = state
        .fleet
        .get(machine)
        .ok_or_else(|| AppError::NotFound(format!("machine '{machine}'")))?;
    let action = Action::parse(action).ok_or_else(|| AppError::NotFound(action.to_string()))?;
    linked.machine().act(file, action).await?;
    Ok(())
}
