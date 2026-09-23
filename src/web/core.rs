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
use crate::quadlet::{QuadletUnit, UnitKind, containerref, discovery, install, naming, writer};
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
    writer::write_atomic(&state.quadlet_dir, &unit.rel_path(), &patched)?;
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
    group: &str,
    contents: &str,
) -> Result<QuadletUnit, AppError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    if !naming::valid_group(group) {
        return Err(
            crate::quadlet::QuadletError::Validation(format!("invalid group '{group}'")).into(),
        );
    }
    if let Some(synced) = synced_destination(state, group) {
        return Err(git_sync_conflict(&synced));
    }
    // Podman keys the generated service off the bare file name, so it must be
    // unique across the whole group tree -- not just the target directory.
    if discovery::find_in_tree(&state.quadlet_dir, file_name).is_some() {
        return Err(crate::quadlet::QuadletError::Validation(format!(
            "{file_name} already exists"
        ))
        .into());
    }
    let rel_path = naming::compose_rel_path(group, file_name);
    writer::write_atomic(&state.quadlet_dir, &rel_path, contents)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = %rel_path, "quadlet created");
    Ok(discovery::load_by_name(
        state.quadlet_dir.as_path(),
        file_name,
    )?)
}

/// Points an existing `.container` quadlet at `pod_file_name` by patching its
/// `[Container]` `Pod=` line and rewriting the file in place -- the "New Pod"
/// and "Edit Pod" pages' follow-up step for each already-defined container
/// the user picked to attach. Rejects non-Container kinds. No CSRF check
/// here: this is only ever called from within a handler (`handlers::pods`)
/// that already verified the one submitted token before running a batch of
/// these.
///
/// Podman only places a container into a pod when it's (re)created, so a
/// quadlet already running under its old `Pod=` (or none) keeps running
/// there until it's next started -- the file change alone doesn't move it.
/// `restart_if_running` closes that gap: when true and the container is
/// currently active, it's restarted right after the reload so it rejoins the
/// right pod immediately instead of on some later, unrelated restart.
pub async fn attach_container_to_pod(
    state: &AppState,
    file_name: &str,
    pod_file_name: &str,
    restart_if_running: bool,
) -> Result<(), AppError> {
    let unit = discovery::load_by_name(state.quadlet_dir.as_path(), file_name)?;
    if unit.kind != UnitKind::Container {
        return Err(crate::quadlet::QuadletError::Validation(format!(
            "{file_name} is not a container"
        ))
        .into());
    }
    let patched = containerref::set_pod(&unit.raw, Some(pod_file_name));
    if patched == unit.raw {
        return Ok(());
    }
    writer::write_atomic(&state.quadlet_dir, &unit.rel_path(), &patched)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = %unit.file_name, pod = pod_file_name, "container attached to pod");

    if restart_if_running {
        let service = unit.service_name();
        let status = state.systemd.status(&service).await?;
        if status.is_active() {
            state.systemd.restart(&service).await?;
            let status = state.systemd.status(&service).await?;
            let _ = state
                .events
                .send(DashboardEvent::Status { service, status });
            tracing::info!(file = %unit.file_name, pod = pod_file_name, "container restarted to join pod");
        }
    }
    Ok(())
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
    // Resolve the file's group so the write lands in its subdirectory rather
    // than at the root.
    let rel_path = discovery::load_by_name(state.quadlet_dir.as_path(), file_name)?.rel_path();
    writer::write_atomic(&state.quadlet_dir, &rel_path, contents)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = %rel_path, "quadlet edited");
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
    let rel_path = discovery::load_by_name(state.quadlet_dir.as_path(), file_name)?.rel_path();
    writer::delete(&state.quadlet_dir, &rel_path)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = %rel_path, "quadlet deleted");
    Ok(())
}

/// Moves a quadlet file into a different group directory (or to the root when
/// `new_group` is empty). The generated service is byte-identical and keeps
/// its name -- only the file's path changes -- so this is a rename plus a
/// `daemon-reload` for the generator to re-scan, with no enable/disable
/// handling. A no-op (returns the unit unchanged) when the group already matches.
pub async fn move_unit(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    file_name: &str,
    new_group: &str,
) -> Result<QuadletUnit, AppError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    let new_group = new_group.trim();
    if !naming::valid_group(new_group) {
        return Err(crate::quadlet::QuadletError::Validation(format!(
            "invalid group '{new_group}': use path segments of letters, digits, '_', '-', '.'; no '..'"
        ))
        .into());
    }
    let unit = discovery::load_by_name(state.quadlet_dir.as_path(), file_name)?;
    if unit.group == new_group {
        return Ok(unit);
    }
    if let Some(synced) = synced_destination(state, new_group) {
        return Err(git_sync_conflict(&synced));
    }
    let to_rel = naming::compose_rel_path(new_group, file_name);
    writer::move_file(&state.quadlet_dir, &unit.rel_path(), &to_rel)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(file = file_name, from = %unit.group, to = new_group, "quadlet moved");
    Ok(discovery::load_by_name(
        state.quadlet_dir.as_path(),
        file_name,
    )?)
}

/// Re-parents a whole group directory: `<quadlet_dir>/<group>` becomes
/// `<quadlet_dir>/<new_parent>/<basename(group)>`. Every unit inside moves
/// with it and keeps its service name (podman keys off the bare file name).
/// A rename + `daemon-reload`; a no-op when the parent is unchanged. Rejects
/// moving a group into itself or one of its own descendants.
///
/// When `group` is itself a git-synced directory, the move goes through
/// `GitSyncManager::move_group` instead of a bare `writer::move_dir`, so
/// `GitSyncConfig.group` -- the manager's own lookup key, and what every
/// later poll re-derives the on-disk path from -- moves with it rather than
/// being left pointing at a path that no longer exists. Moving a group that
/// merely sits inside, or is an ancestor of, a synced directory is still
/// rejected: only the sync's own directory can be moved this way.
pub async fn move_group_dir(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    group: &str,
    new_parent: &str,
) -> Result<(), AppError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    let group = group.trim().trim_matches('/');
    let new_parent = new_parent.trim().trim_matches('/');
    if group.is_empty() || !naming::valid_group(group) {
        return Err(bad_group(group));
    }
    if !naming::valid_group(new_parent) {
        return Err(bad_group(new_parent));
    }
    if new_parent == group || new_parent.starts_with(&format!("{group}/")) {
        return Err(crate::quadlet::QuadletError::Validation(
            "cannot move a group into itself or one of its own subgroups".into(),
        )
        .into());
    }
    let is_synced_group = is_synced_group(state, group);
    if !is_synced_group && let Some(synced) = synced_source(state, group) {
        return Err(git_sync_conflict(&synced));
    }
    if let Some(synced) = synced_destination(state, new_parent) {
        return Err(git_sync_conflict(&synced));
    }
    let to = naming::compose_rel_path(new_parent, naming::basename(group));
    if to == group {
        return Ok(()); // already at this parent
    }
    if is_synced_group {
        state
            .git_sync
            .move_group(group, &to, &state.quadlet_dir)
            .await?;
    } else {
        writer::move_dir(&state.quadlet_dir, group, &to)?;
    }
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(from = group, to = %to, "group directory moved");
    Ok(())
}

fn bad_group(g: &str) -> AppError {
    crate::quadlet::QuadletError::Validation(format!(
        "invalid group '{g}': use path segments of letters, digits, '_', '-', '.'; no '..'"
    ))
    .into()
}

/// Whether `group` names a destination a configured git-sync already
/// manages -- exactly its directory, or somewhere inside it. Guards every
/// place a unit or group can land: the next sync's `reset --hard` would
/// just discard anything dropped there by hand. Returns the overlapping
/// sync's group path (for the error message) rather than a bare bool.
fn synced_destination(state: &AppState, group: &str) -> Option<String> {
    destination_overlap(&state.git_sync.synced_groups(), group)
}

/// Whether relocating or renaming `group` itself would disturb a configured
/// git-sync's path tracking: `group` *is* a sync's directory, sits inside
/// one, or contains one as a descendant (any of which would carry the
/// synced checkout to a new path that `GitSyncConfig.group` no longer
/// names, so the next poll can't find it). Callers special-case the "*is*
/// a sync's directory" case via [`is_synced_group`] -- that one's allowed,
/// routed through `GitSyncManager::move_group` instead of rejected outright.
fn synced_source(state: &AppState, group: &str) -> Option<String> {
    source_overlap(&state.git_sync.synced_groups(), group)
}

/// Whether `group` is exactly a configured git-sync's own directory -- not
/// one of its subdirectories, and not an ancestor merely containing one.
/// The specific `synced_source` overlap that `move_group_dir`/`rename_group`
/// are allowed to act on, by moving/renaming it through
/// `GitSyncManager::move_group` (which keeps `GitSyncConfig.group` pointed
/// at the right path) rather than a bare `writer::move_dir`.
fn is_synced_group(state: &AppState, group: &str) -> bool {
    state.git_sync.synced_groups().iter().any(|g| g == group)
}

/// The pure check behind [`synced_destination`], split out so it's testable
/// without an `AppState` (which needs a live D-Bus session to construct).
fn destination_overlap(synced_groups: &[String], group: &str) -> Option<String> {
    synced_groups
        .iter()
        .find(|synced| group == synced.as_str() || group.starts_with(&format!("{synced}/")))
        .cloned()
}

/// The pure check behind [`synced_source`]; see [`destination_overlap`].
fn source_overlap(synced_groups: &[String], group: &str) -> Option<String> {
    synced_groups
        .iter()
        .find(|synced| {
            group == synced.as_str()
                || group.starts_with(&format!("{synced}/"))
                || synced.starts_with(&format!("{group}/"))
        })
        .cloned()
}

fn git_sync_conflict(synced_group: &str) -> AppError {
    crate::quadlet::QuadletError::Validation(format!(
        "'{synced_group}' is synced from a git repository (see the Git Sync page) and is \
         managed by the remote -- it can't be used as a move/create target, and a directory \
         inside it or containing it can't be moved or renamed either. Move '{synced_group}' \
         itself instead."
    ))
    .into()
}

/// Renames a group's last path segment, keeping it under the same parent:
/// `media/arr` + `series` -> `media/series`. A `rename` on disk (contents
/// move with it) + `daemon-reload`; a no-op when the name is unchanged. Like
/// [`move_group_dir`], a git-synced group's own directory is allowed to be
/// renamed this way (via `GitSyncManager::move_group`); one that merely sits
/// inside or contains a sync is still rejected.
pub async fn rename_group(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    group: &str,
    new_name: &str,
) -> Result<(), AppError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    let group = group.trim().trim_matches('/');
    let new_name = new_name.trim();
    if group.is_empty() || !naming::valid_group(group) {
        return Err(bad_group(group));
    }
    // a single segment only -- `valid_group` of a `/`-free string checks the rest
    if new_name.is_empty() || new_name.contains('/') || !naming::valid_group(new_name) {
        return Err(crate::quadlet::QuadletError::Validation(format!(
            "invalid group name '{new_name}'"
        ))
        .into());
    }
    let parent = group.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let to = naming::compose_rel_path(parent, new_name);
    if to == group {
        return Ok(());
    }
    let is_synced_group = is_synced_group(state, group);
    if !is_synced_group && let Some(synced) = synced_source(state, group) {
        return Err(git_sync_conflict(&synced));
    }
    if is_synced_group {
        state
            .git_sync
            .move_group(group, &to, &state.quadlet_dir)
            .await?;
    } else {
        writer::move_dir(&state.quadlet_dir, group, &to)?;
    }
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(from = group, to = %to, "group renamed");
    Ok(())
}

/// Deletes a group directory (and any empty subdirectory tree under it).
/// Refuses when a quadlet file still lives anywhere below it -- the units must
/// be moved or deleted first.
pub async fn delete_group(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    group: &str,
) -> Result<(), AppError> {
    if !auth::csrf::verify(session, csrf_token).await {
        return Err(AppError::Csrf);
    }
    let group = group.trim().trim_matches('/');
    if group.is_empty() || !naming::valid_group(group) {
        return Err(bad_group(group));
    }
    if discovery::group_has_units(&state.quadlet_dir, group) {
        return Err(crate::quadlet::QuadletError::Validation(format!(
            "group '{group}' still contains units — move or delete them first"
        ))
        .into());
    }
    writer::delete_dir(&state.quadlet_dir, group)?;
    state.systemd.reload().await?;
    let _ = state.events.send(DashboardEvent::UnitsChanged);
    tracing::info!(group, "group deleted");
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

    fn synced(groups: &[&str]) -> Vec<String> {
        groups.iter().map(|g| g.to_string()).collect()
    }

    #[test]
    fn destination_overlap_flags_the_synced_dir_and_its_subdirs() {
        let s = synced(&["media/arr"]);
        assert_eq!(
            destination_overlap(&s, "media/arr").as_deref(),
            Some("media/arr")
        );
        assert_eq!(
            destination_overlap(&s, "media/arr/hd").as_deref(),
            Some("media/arr")
        );
        // A sibling that merely shares a prefix is not "inside" it.
        assert_eq!(destination_overlap(&s, "media/arr-extra"), None);
        assert_eq!(destination_overlap(&s, "media"), None);
        assert_eq!(destination_overlap(&s, ""), None);
    }

    #[test]
    fn source_overlap_also_catches_moving_an_ancestor_of_a_synced_dir() {
        let s = synced(&["media/arr"]);
        // Relocating the synced dir itself.
        assert_eq!(
            source_overlap(&s, "media/arr").as_deref(),
            Some("media/arr")
        );
        // Relocating something inside it.
        assert_eq!(
            source_overlap(&s, "media/arr/hd").as_deref(),
            Some("media/arr")
        );
        // Relocating an ancestor would carry the synced dir along with it.
        assert_eq!(source_overlap(&s, "media").as_deref(), Some("media/arr"));
        // An unrelated group is untouched.
        assert_eq!(source_overlap(&s, "other"), None);
    }
}
