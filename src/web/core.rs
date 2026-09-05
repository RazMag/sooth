//! Kind-agnostic business logic shared by every section (Containers, Pods,
//! Volumes, Networks, Images, and the generic `/units` fallback). Nothing
//! here knows about HTTP -- it takes plain values and returns `AppError` on
//! failure, so every section's thin Axum handlers stay identical regardless
//! of which kind they're wiring up.

use tower_sessions::Session;
use tracing::Instrument;

use crate::auth;
use crate::config::AppState;
use crate::error::{AppError, FragmentError};
use crate::events::DashboardEvent;
use crate::quadlet::{QuadletUnit, UnitKind, discovery, writer};
use crate::systemd::UnitStatus;

/// The section a unit belongs to, and therefore the URL prefix all of its
/// pages live under. The single source of truth for building any
/// unit-specific URL -- centralizing it here is what makes the
/// file-name-vs-service-name class of bug structurally hard to reintroduce,
/// since there's exactly one place that turns a unit into a URL.
pub fn section_path(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Container => "/containers",
        UnitKind::Pod => "/pods",
        UnitKind::Volume => "/volumes",
        UnitKind::Network => "/networks",
        UnitKind::Image | UnitKind::Build => "/images",
        UnitKind::Kube => "/units",
    }
}

pub fn unit_url(unit: &QuadletUnit) -> String {
    format!("{}/{}", section_path(unit.kind), unit.file_name)
}

/// Loads every quadlet file matching one of `kinds` along with its live
/// systemd status, for a section's list/overview page.
pub async fn load_units_for_kinds(
    state: &AppState,
    kinds: &[UnitKind],
) -> Result<Vec<(QuadletUnit, UnitStatus)>, AppError> {
    let all = discovery::load_all(&state.quadlet_dir)?;
    let mut out = Vec::new();
    for unit in all.into_iter().filter(|u| kinds.contains(&u.kind)) {
        let status = if unit.is_template() {
            UnitStatus::not_found()
        } else {
            state.systemd.status(&unit.service_name()).await?
        };
        out.push((unit, status));
    }
    Ok(out)
}

pub struct ActionOutcome {
    pub unit: QuadletUnit,
    pub status: UnitStatus,
}

/// Verifies CSRF, resolves the quadlet file to its systemd service name,
/// runs the requested systemd action inside a span carrying unit/action
/// context, broadcasts the resulting status to any open tab, and returns
/// both the unit and its freshly re-fetched status.
pub async fn execute_action(
    state: &AppState,
    session: &Session,
    file_name: &str,
    csrf_token: &str,
    action: &'static str,
) -> Result<ActionOutcome, FragmentError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(FragmentError(AppError::Csrf));
    }
    let unit = discovery::load_by_name(state.quadlet_dir.as_path(), file_name)?;
    let service = unit.service_name();

    let span = tracing::info_span!("unit_action", unit = %service, action);
    async {
        let result = match action {
            "start" => state.systemd.start(&service).await,
            "stop" => state.systemd.stop(&service).await,
            "restart" => state.systemd.restart(&service).await,
            "enable" => state.systemd.enable(&service).await,
            "disable" => state.systemd.disable(&service).await,
            _ => unreachable!("action is one of the fixed route names in handlers/unit_ops.rs"),
        };
        result.map_err(FragmentError::from)
    }
    .instrument(span)
    .await?;

    let status = state.systemd.status(&service).await?;
    let _ = state.events.send(DashboardEvent::Status {
        service: service.clone(),
        status: status.clone(),
    });
    Ok(ActionOutcome { unit, status })
}

/// Verifies CSRF, atomically writes a brand-new quadlet file (rejecting if
/// one already exists -- a guided "create" implies a new file), reloads
/// systemd, and broadcasts the change. Returns the freshly loaded unit so
/// callers can redirect via `unit_url`.
pub async fn create_unit(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    file_name: &str,
    contents: &str,
) -> Result<QuadletUnit, AppError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    if state.quadlet_dir.join(file_name).exists() {
        return Err(crate::quadlet::QuadletError::Validation(format!(
            "{file_name} already exists"
        ))
        .into());
    }
    writer::write_atomic(&state.quadlet_dir, file_name, contents)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = file_name, "quadlet created");
    Ok(discovery::load_by_name(
        state.quadlet_dir.as_path(),
        file_name,
    )?)
}

/// Same tail as `create_unit`, but overwrites an existing file -- used by
/// the raw-textarea edit flow (which stays kind-agnostic for every kind).
pub async fn edit_unit(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    file_name: &str,
    contents: &str,
) -> Result<QuadletUnit, AppError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    writer::write_atomic(&state.quadlet_dir, file_name, contents)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = file_name, "quadlet edited");
    Ok(discovery::load_by_name(
        state.quadlet_dir.as_path(),
        file_name,
    )?)
}

pub async fn delete_unit(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    file_name: &str,
) -> Result<(), AppError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    writer::delete(&state.quadlet_dir, file_name)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = file_name, "quadlet deleted");
    Ok(())
}
