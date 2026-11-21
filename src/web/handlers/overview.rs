//! The Overview home page (`/`) and its live-refresh fragment. Every
//! machine in scope (`?m=`, see [`Scope`]) gets a card, and its failed units
//! and silence count towards the headline and "Needs you".

use std::collections::BTreeMap;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use futures_util::future::join_all;

use crate::config::AppState;
use crate::error::{AppError, FragmentError, PageError};
use crate::fleet::{Linked, Listing, Reach};
use crate::machine::Facts;
use crate::quadlet::gitsync::SyncState;
use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;
use crate::web::core;
use crate::web::handlers::services::compute_stats;
use crate::web::scope::{Scope, ScopeQuery};
use crate::web::templates::list::remote_unit_url;
use crate::web::templates::overview::{
    Answer, FailedUnit, MachineCard, MachineInfo, MissingSecret, Overview, OverviewPage,
    SyncSummary,
};
use crate::web::templates::{self, now_usec, status_detail};

const KINDS: &[UnitKind] = templates::services::SPEC.kinds;

/// This hub's own share: its card, and the problems only it can see so far
/// (secrets, image tags and health checks are read locally).
struct LocalPart {
    card: MachineCard,
    units: Vec<(QuadletUnit, UnitStatus)>,
    failed: Vec<FailedUnit>,
    missing_secrets: Vec<MissingSecret>,
    newer_tags: Vec<String>,
    unhealthy: Vec<FailedUnit>,
}

async fn local_part(state: &AppState, now: u64) -> Result<LocalPart, AppError> {
    let (units, all) = core::load_units_and_siblings(state, KINDS).await?;
    let failed = units
        .iter()
        .filter(|(_, s)| s.is_failed())
        .map(|(u, s)| {
            let href = core::unit_url(u);
            FailedUnit {
                name: u.file_name.clone(),
                logs: format!("{href}/logs"),
                href,
                detail: status_detail(s, now),
                machine: None,
            }
        })
        .collect();

    // Each unset secret once, with how many units reference it.
    let mut secrets: BTreeMap<String, usize> = BTreeMap::new();
    for refs in core::missing_refs(state, &units).await.values() {
        for name in &refs.secrets {
            *secrets.entry(name.clone()).or_default() += 1;
        }
    }
    let missing_secrets = secrets
        .into_iter()
        .map(|(name, units)| MissingSecret { name, units })
        .collect();

    let mut newer_tags: Vec<String> = core::newer_tags(state, &units, &all).into_keys().collect();
    newer_tags.sort();

    let checks = crate::containerhealth::health_of(units.iter().map(|(u, _)| u)).await;
    let unhealthy = units
        .iter()
        .filter(|(u, s)| {
            s.is_active()
                && checks.get(&u.file_name)
                    == Some(&crate::containerhealth::ContainerHealth::Unhealthy)
        })
        .map(|(u, _)| {
            let href = core::unit_url(u);
            FailedUnit {
                name: u.file_name.clone(),
                logs: format!("{href}#logs"),
                href,
                detail: String::new(),
                machine: None,
            }
        })
        .collect();

    let facts = state.machine.facts().await?;
    let info = MachineInfo::from(&facts);
    let card = MachineCard {
        name: info.hostname.clone(),
        href: templates::NavItem::Machine.href().to_string(),
        local: true,
        info: Some(info),
        stats: Some(compute_stats(&units)),
        health: Some(facts.health),
        answer: Answer::Live,
    };
    Ok(LocalPart {
        card,
        units,
        failed,
        missing_secrets,
        newer_tags,
        unhealthy,
    })
}

/// Another machine's share: its card and units, from what it answers now
/// or last answered.
struct RemotePart {
    card: MachineCard,
    units: Vec<(QuadletUnit, UnitStatus)>,
    failed: Vec<FailedUnit>,
}

/// What `linked` last said about itself, asking it if the hub hasn't heard
/// since it came back.
async fn facts_of(state: &AppState, linked: &Linked, live: bool) -> Option<Facts> {
    let reach = match state.fleet.seen(&linked.entry.id) {
        Some(seen) if seen.reach.is_online() => seen.reach,
        _ if live => state.fleet.check(linked).await,
        _ => return None,
    };
    match reach {
        Reach::Online { facts, .. } => Some(facts),
        Reach::Offline { .. } => None,
    }
}

async fn remote_part(state: &AppState, linked: Linked, now: u64) -> RemotePart {
    let listing = state.fleet.listing(&linked, KINDS).await;
    let facts = facts_of(state, &linked, listing.is_live()).await;
    let entry = &linked.entry;
    let units = listing.units().map(|(u, _)| u.clone());
    let answer = match &listing {
        Listing::Live(_) => Answer::Live,
        Listing::Stale { as_of, reason, .. } => Answer::Stale {
            ago: templates::selfupdate::ago(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(*as_of),
            ),
            reason: reason.clone(),
        },
        Listing::Unreachable { reason } => Answer::Unreachable {
            reason: reason.clone(),
        },
    };
    let failed = units
        .iter()
        .flatten()
        .filter(|(_, s)| s.is_failed())
        .map(|(u, s)| {
            let href = remote_unit_url(&entry.id, &u.file_name);
            FailedUnit {
                name: u.file_name.clone(),
                logs: format!("{href}#logs"),
                href,
                detail: status_detail(s, now),
                machine: Some(entry.name.clone()),
            }
        })
        .collect();
    RemotePart {
        card: MachineCard {
            name: entry.name.clone(),
            href: format!("/services?m={}", entry.id),
            local: false,
            info: facts.as_ref().map(MachineInfo::from),
            stats: units.as_deref().map(compute_stats),
            health: facts.map(|f| f.health),
            answer,
        },
        units: units.unwrap_or_default(),
        failed,
    }
}

async fn gather(state: &AppState, scope: &Scope) -> Result<Overview, AppError> {
    let now = now_usec();
    let remotes = join_all(
        scope
            .remotes(&state.fleet)
            .into_iter()
            .map(|linked| remote_part(state, linked, now)),
    );
    let local = async {
        if scope.includes_local() {
            local_part(state, now).await.map(Some)
        } else {
            Ok(None)
        }
    };
    let (local, remotes) = tokio::join!(local, remotes);
    let local = local?;

    let mut units: Vec<(QuadletUnit, UnitStatus)> = Vec::new();
    let mut machines = Vec::new();
    let mut failed = Vec::new();
    let mut overview_local = None;
    if let Some(l) = local {
        units.extend(l.units);
        machines.push(l.card);
        failed.extend(l.failed);
        overview_local = Some((l.missing_secrets, l.newer_tags, l.unhealthy));
    }
    for r in remotes {
        units.extend(r.units);
        machines.push(r.card);
        failed.extend(r.failed);
    }
    let (missing_secrets, newer_tags, unhealthy) = match overview_local {
        Some((secrets, tags, unhealthy)) => (secrets, Some(tags), unhealthy),
        None => (Vec::new(), None, Vec::new()),
    };

    let snapshot = state.git_sync.snapshot();
    let sync = SyncSummary {
        repos: snapshot.len(),
        attention: snapshot
            .iter()
            .filter(|(_, s)| matches!(s.state, SyncState::Error(_)))
            .count(),
    };

    Ok(Overview {
        machines,
        stats: compute_stats(&units),
        failed,
        missing_secrets,
        newer_tags,
        sync,
        unhealthy,
        services_href: scope.link("/services"),
    })
}

/// How many Activity entries the Overview shows.
const ACTIVITY_SHOWN: usize = 30;

fn now_secs() -> u64 {
    now_usec() / 1_000_000
}

/// What redraws the body: this hub's own changes, and -- since other
/// machines don't push theirs yet -- a machine coming or going and every
/// 30 seconds while any is in scope.
fn refresh_trigger(scope: &Scope, state: &AppState) -> &'static str {
    if scope.remotes(&state.fleet).is_empty() {
        "sse:units-changed, sse:any-status delay:500ms, sse:image-tags-changed"
    } else {
        "sse:units-changed, sse:any-status delay:500ms, sse:image-tags-changed, sse:machines-changed, machine-units-changed from:body, every 30s"
    }
}

pub async fn page(
    State(state): State<AppState>,
    Query(query): Query<ScopeQuery>,
) -> Result<impl IntoResponse, PageError> {
    let scope = Scope::resolve(&state.fleet, query.m.as_deref())?;
    let overview = gather(&state, &scope).await?;
    let activity = state.activity.recent(ACTIVITY_SHOWN);
    let body_route = scope.link("/overview/body");
    Ok(templates::overview::overview_page(&OverviewPage {
        overview: &overview,
        activity: &activity,
        now: now_secs(),
        scopes: scope.choices(&state.fleet, "/"),
        body_route: &body_route,
        refresh: refresh_trigger(&scope, &state),
        creates: scope.includes_local(),
        health: state.health.get(),
    }))
}

pub async fn activity(State(state): State<AppState>) -> impl IntoResponse {
    templates::overview::activity_items(&state.activity.recent(ACTIVITY_SHOWN), now_secs())
}

pub async fn body(
    State(state): State<AppState>,
    Query(query): Query<ScopeQuery>,
) -> Result<impl IntoResponse, FragmentError> {
    let scope = Scope::resolve(&state.fleet, query.m.as_deref())?;
    Ok(templates::overview::overview_body(
        &gather(&state, &scope).await?,
    ))
}
