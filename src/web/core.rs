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
use crate::quadlet::autoupdate::{self, AutoUpdateMode};
use crate::quadlet::{QuadletUnit, UnitKind, discovery, install, writer};
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

/// The list/overview page to land on after a unit of this kind is deleted.
/// Differs from [`section_path`] for Containers and Pods: those have no
/// standalone list route (`/containers` / `/pods` are POST-only create
/// targets), they live on the combined Services home page, so a bare
/// redirect to `section_path` there 404s.
pub fn section_index_path(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Container | UnitKind::Pod => "/",
        UnitKind::Volume => "/volumes",
        UnitKind::Network => "/networks",
        UnitKind::Image | UnitKind::Build => "/images",
        UnitKind::Kube => "/units",
    }
}

/// Loads every quadlet file matching one of `kinds` along with its live
/// systemd status, for a section's list/overview page.
pub async fn load_units_for_kinds(
    state: &AppState,
    kinds: &[UnitKind],
) -> Result<Vec<(QuadletUnit, UnitStatus)>, AppError> {
    Ok(load_units_and_siblings(state, kinds).await?.0)
}

/// Like [`load_units_for_kinds`], but also returns *every* quadlet on disk
/// (all kinds) alongside the filtered+status list -- for list columns that
/// resolve cross-unit references (the Volumes/Networks "Used by" column).
/// The full enumeration happens either way, so this adds no I/O.
pub async fn load_units_and_siblings(
    state: &AppState,
    kinds: &[UnitKind],
) -> Result<(Vec<(QuadletUnit, UnitStatus)>, Vec<QuadletUnit>), AppError> {
    let all = discovery::load_all(&state.quadlet_dir)?;
    let mut out = Vec::new();
    for unit in all.iter().filter(|u| kinds.contains(&u.kind)) {
        let status = if unit.is_template() {
            UnitStatus::not_found()
        } else {
            state.systemd.status(&unit.service_name()).await?
        };
        out.push((unit.clone(), status));
    }
    Ok((out, all))
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
    let mut unit = discovery::load_by_name(state.quadlet_dir.as_path(), file_name)?;
    let service = unit.service_name();
    let is_autostart = matches!(action, "enable" | "disable");

    let span = tracing::info_span!("unit_action", unit = %service, action);
    async {
        match action {
            "start" => state.systemd.start(&service).await?,
            "stop" => state.systemd.stop(&service).await?,
            "restart" => state.systemd.restart(&service).await?,
            // Podman's generated `.service` units live under a systemd
            // generator dir, which `EnableUnitFiles` refuses outright -- so
            // "enable"/"disable" is a patch of the quadlet's `[Install]`
            // section plus a reload, not a D-Bus enable call.
            "enable" => set_autostart(state, &unit, true).await?,
            "disable" => set_autostart(state, &unit, false).await?,
            _ => unreachable!("action is one of the fixed route names in handlers/unit_ops.rs"),
        }
        Ok::<(), FragmentError>(())
    }
    .instrument(span)
    .await?;

    if is_autostart {
        // The file on disk just changed; re-read it so the re-rendered
        // action row shows the opposite button, and tell every open list.
        unit = discovery::load_by_name(state.quadlet_dir.as_path(), file_name)?;
        let _ = state.events.send(DashboardEvent::UnitsChanged);
    }

    let status = state.systemd.status(&service).await?;
    let _ = state.events.send(DashboardEvent::Status {
        service: service.clone(),
        status: status.clone(),
    });
    Ok(ActionOutcome { unit, status })
}

/// The rootless "enable"/"disable": add or remove a managed `[Install]` /
/// `WantedBy=default.target` in the quadlet file (validated + written
/// atomically, same as any edit) and reload so the generator picks it up. A
/// no-op when the file is already in the requested state.
async fn set_autostart(
    state: &AppState,
    unit: &QuadletUnit,
    enabled: bool,
) -> Result<(), FragmentError> {
    let patched = install::set_enabled(&unit.raw, enabled);
    if patched == unit.raw {
        return Ok(());
    }
    writer::write_atomic(&state.quadlet_dir, &unit.file_name, &patched)?;
    state.systemd.reload().await?;
    tracing::info!(file = %unit.file_name, enabled, "autostart toggled via [Install]");
    Ok(())
}

/// Sets (or clears, with `mode == None`) the `[Container]` `AutoUpdate=` policy
/// on a container quadlet: patch the raw file text, write it atomically (same
/// validation as any edit), and reload so the generator re-labels the service.
/// A no-op when the file is already in the requested state. Rejects non-Container
/// kinds -- `AutoUpdate=` is a `[Container]` key.
pub async fn set_container_autoupdate(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    file_name: &str,
    mode: Option<AutoUpdateMode>,
) -> Result<QuadletUnit, AppError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    let unit = discovery::load_by_name(state.quadlet_dir.as_path(), file_name)?;
    if unit.kind != UnitKind::Container {
        return Err(crate::quadlet::QuadletError::Validation(
            "auto-update is a container-only setting".into(),
        )
        .into());
    }
    let patched = autoupdate::set_autoupdate(&unit.raw, mode);
    if patched == unit.raw {
        return Ok(unit);
    }
    writer::write_atomic(&state.quadlet_dir, &unit.rel_path(), &patched)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = %unit.file_name, ?mode, "auto-update policy set");
    Ok(discovery::load_by_name(
        state.quadlet_dir.as_path(),
        file_name,
    )?)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_index_path_avoids_the_post_only_routes() {
        // Containers/Pods have no GET list page -- they live on `/`.
        assert_eq!(section_index_path(UnitKind::Container), "/");
        assert_eq!(section_index_path(UnitKind::Pod), "/");
        // The rest have real list pages.
        assert_eq!(section_index_path(UnitKind::Volume), "/volumes");
        assert_eq!(section_index_path(UnitKind::Network), "/networks");
        assert_eq!(section_index_path(UnitKind::Image), "/images");
        assert_eq!(section_index_path(UnitKind::Build), "/images");
        assert_eq!(section_index_path(UnitKind::Kube), "/units");
    }
}
