//! The Overview home page (`/`) and its live-refresh fragment.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::response::IntoResponse;

use crate::config::AppState;
use crate::error::{AppError, FragmentError, PageError};
use crate::quadlet::gitsync::SyncState;
use crate::web::core;
use crate::web::handlers::services::compute_stats;
use crate::web::templates::overview::{
    FailedUnit, MachineInfo, MissingSecret, Overview, SyncSummary,
};
use crate::web::templates::{self, now_usec, status_detail};

async fn gather(state: &AppState) -> Result<Overview, AppError> {
    let kinds = templates::services::SPEC.kinds;
    let (units, all) = core::load_units_and_siblings(state, kinds).await?;
    let now = now_usec();
    let failed = units
        .iter()
        .filter(|(_, s)| s.is_failed())
        .map(|(u, s)| FailedUnit {
            name: u.file_name.clone(),
            href: core::unit_url(u),
            detail: status_detail(s, now),
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
        .map(|(u, _)| FailedUnit {
            name: u.file_name.clone(),
            href: core::unit_url(u),
            detail: String::new(),
        })
        .collect();

    let snapshot = state.git_sync.snapshot();
    let sync = SyncSummary {
        repos: snapshot.len(),
        attention: snapshot
            .iter()
            .filter(|(_, s)| matches!(s.state, SyncState::Error(_)))
            .count(),
    };

    let facts = state.machine.facts().await?;
    Ok(Overview {
        machine: MachineInfo::from(&facts),
        stats: compute_stats(&units),
        failed,
        missing_secrets,
        newer_tags,
        sync,
        health: facts.health,
        unhealthy,
    })
}

/// How many Activity entries the Overview shows.
const ACTIVITY_SHOWN: usize = 30;

fn now_secs() -> u64 {
    now_usec() / 1_000_000
}

pub async fn page(State(state): State<AppState>) -> Result<impl IntoResponse, PageError> {
    let overview = gather(&state).await?;
    let activity = state.activity.recent(ACTIVITY_SHOWN);
    Ok(templates::overview::overview_page(
        &overview,
        &activity,
        now_secs(),
    ))
}

pub async fn activity(State(state): State<AppState>) -> impl IntoResponse {
    templates::overview::activity_items(&state.activity.recent(ACTIVITY_SHOWN), now_secs())
}

pub async fn body(State(state): State<AppState>) -> Result<impl IntoResponse, FragmentError> {
    Ok(templates::overview::overview_body(&gather(&state).await?))
}
