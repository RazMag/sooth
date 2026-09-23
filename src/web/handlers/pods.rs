//! The dedicated Pod flow. Unlike every other section (all sharing
//! `raw_create`'s bare stem-plus-INI create form and `edit_delete`'s bare
//! raw-INI edit form), both creating and editing a pod also offer picking
//! already-defined containers/networks/volumes to attach, and defining
//! brand-new containers inline -- Quadlet's pod-membership model is inverted
//! for containers specifically (a `.pod` file never lists them; each
//! `.container` file opts in via `Pod=<podfile>`), so a plain raw-INI pod
//! form has nowhere to put that; networks and volumes *do* live in the
//! pod's own `[Pod]` section (`Network=`/`Volume=`), but picking one from a
//! list beats hand-typing the line too. `/units/new` picking "Pod" is
//! untouched and still goes through `raw_create`; this module owns `/pods`,
//! `/pods/new`, and (separately from the shared `mount_unit_routes` loop --
//! see `routes.rs`) `/pods/{file}/edit`.
//!
//! The submitted form is an untyped `HashMap<String, String>`, not a
//! `#[derive(Deserialize)]` struct: `axum::Form` is backed by
//! `serde_urlencoded`, which has no concept of indexed/bracketed field names
//! and so can't express "N independently-named rows of 3 fields each" (the
//! "New containers" field -- see `parse_new_containers`) as a typed `Vec`.
//! Every field, including the simple single-value ones, is read via
//! `map.get(...)` as a result.

use std::collections::HashMap;

use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use tower_sessions::Session;

use crate::config::AppState;
use crate::error::{AppError, PageError};
use crate::quadlet::{
    QuadletUnit, UnitKind, containerref, discovery, envfile, iniedit, naming, refs, writer,
};
use crate::web::templates::pods::{
    ContainerOption, EditPodPage, NewContainerRow, NewNetworkRow, NewPodPage, NewVolumeRow,
    ResourceOption,
};
use crate::web::{core, templates};

/// Every non-template `.container` quadlet on disk the picker can offer --
/// its current `Pod=` (if any) is surfaced so re-picking an already-attached
/// container is an informed move, not a silent one. `exclude_pod` drops
/// containers already members of that pod (the Edit page's case: attaching
/// one of the pod's own current members isn't meaningful); `None` for the
/// New Pod page, where nothing could already be a member of a pod that
/// doesn't exist yet.
fn container_options(all: &[QuadletUnit], exclude_pod: Option<&str>) -> Vec<ContainerOption> {
    all.iter()
        .filter(|u| u.kind == UnitKind::Container && !u.is_template())
        .filter_map(|u| {
            let current_pod = u
                .section("Container")
                .and_then(|s| s.get("Pod"))
                .map(str::to_string);
            if exclude_pod.is_some() && current_pod.as_deref() == exclude_pod {
                return None;
            }
            Some(ContainerOption {
                file_name: u.file_name.clone(),
                group: u.group.clone(),
                image: u
                    .section("Container")
                    .and_then(|s| s.get("Image"))
                    .map(str::to_string),
                current_pod,
            })
        })
        .collect()
}

/// Every non-template quadlet of `kind` on disk the network/volume pickers
/// can offer, minus anything in `exclude` (the Edit page's case: a network
/// or volume the pod's own `[Pod]` section already references, per
/// `refs::pod_own_refs` -- picking it again would be a no-op). `exclude` is
/// empty for the New Pod page, where the pod (and so its refs) don't exist
/// yet.
fn resource_options(
    all: &[QuadletUnit],
    kind: UnitKind,
    exclude: &[String],
) -> Vec<ResourceOption> {
    all.iter()
        .filter(|u| u.kind == kind && !u.is_template())
        .filter(|u| !exclude.iter().any(|e| e == &u.file_name))
        .map(|u| ResourceOption {
            file_name: u.file_name.clone(),
            group: u.group.clone(),
        })
        .collect()
}

/// Parses a form's staged "attach these" field into the list of file names --
/// shared shape for the existing-containers and existing-networks fields.
fn parse_existing(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// `FILE=DEST` lines from the "attach volume" field -- the volume's file
/// name and the mount path to give it inside the pod's containers (`Volume=`
/// needs a `SOURCE:DEST` pair; unlike a container/network reference, sooth
/// can't infer the destination). 1-based line-number errors, same shape as
/// `envfile::parse_editor_lines`.
fn parse_volume_lines(raw: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Some((file_name, dest)) = t.split_once('=') else {
            return Err(format!("line {}: expected FILE=DEST", i + 1));
        };
        let file_name = file_name.trim();
        let dest = dest.trim();
        if dest.is_empty() {
            return Err(format!("line {}: '{file_name}' needs a mount path", i + 1));
        }
        if dest.contains([':', '\n', '\r']) {
            return Err(format!(
                "line {}: mount path can't contain ':' or a newline",
                i + 1
            ));
        }
        out.push((file_name.to_string(), dest.to_string()));
    }
    Ok(out)
}

/// Resolves one staged file name to a unit of the expected `kind`, or the
/// message to reject the submission with.
fn validate_resource(
    all: &[QuadletUnit],
    file_name: &str,
    kind: UnitKind,
    noun: &str,
) -> Result<(), String> {
    match all.iter().find(|u| u.file_name == file_name) {
        Some(u) if u.kind == kind => Ok(()),
        Some(_) => Err(format!("{file_name} is not a {noun}")),
        None => Err(format!("{file_name} not found")),
    }
}

fn validate_existing(all: &[QuadletUnit], existing: &[String]) -> Result<(), String> {
    existing
        .iter()
        .try_for_each(|f| validate_resource(all, f, UnitKind::Container, "container"))
}

fn validate_networks(all: &[QuadletUnit], networks: &[String]) -> Result<(), String> {
    networks
        .iter()
        .try_for_each(|f| validate_resource(all, f, UnitKind::Network, "network"))
}

fn validate_volumes(all: &[QuadletUnit], volumes: &[(String, String)]) -> Result<(), String> {
    volumes
        .iter()
        .try_for_each(|(f, _)| validate_resource(all, f, UnitKind::Volume, "volume"))
}

/// Folds the staged networks/volumes into the pod's own `[Pod]` section,
/// appending a `Network=`/`Volume=` line for each -- run once, right before
/// the write, on top of whatever the user has in the raw Contents editor.
fn apply_pod_resources(
    contents: &str,
    networks: &[String],
    volumes: &[(String, String)],
) -> String {
    let mut out = contents.to_string();
    for net in networks {
        out = iniedit::patch_line(&out, "Pod", "Network", net, true);
    }
    for (vol, dest) in volumes {
        out = iniedit::patch_line(&out, "Pod", "Volume", &format!("{vol}:{dest}"), true);
    }
    out
}

/// One brand-new container staged via the pod page's "New containers" field
/// -- the raw `newc_{id}_name`/`_contents`/`_env` values from the submitted
/// form, grouped by `id`. `id` carries no meaning beyond "which three fields
/// belong together"; it's whatever `frontend/podnewcontainers.js` assigned
/// when the row was added client-side (or the row's position on a 422
/// redisplay).
struct NewContainerDraft {
    id: u32,
    name: String,
    contents: String,
    env: String,
}

/// Sanity cap on new-container rows per submission -- the form is an
/// untyped `HashMap` (see the module doc comment), so nothing about its size
/// or key shape is bounded by serde; this is insurance, not a realistic
/// usage limit.
const MAX_NEW_CONTAINERS: usize = 20;

/// Groups the submitted form's `newc_{id}_*` keys into drafts, one per
/// distinct numeric `id`, sorted for a deterministic, stable order.
fn parse_new_containers(fields: &HashMap<String, String>) -> Vec<NewContainerDraft> {
    let mut ids: Vec<u32> = fields
        .keys()
        .filter_map(|k| k.strip_prefix("newc_")?.strip_suffix("_name")?.parse().ok())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.truncate(MAX_NEW_CONTAINERS);

    ids.into_iter()
        .map(|id| NewContainerDraft {
            id,
            name: fields
                .get(&format!("newc_{id}_name"))
                .cloned()
                .unwrap_or_default(),
            contents: fields
                .get(&format!("newc_{id}_contents"))
                .cloned()
                .unwrap_or_default(),
            env: fields
                .get(&format!("newc_{id}_env"))
                .cloned()
                .unwrap_or_default(),
        })
        .collect()
}

/// A validated, write-ready new container -- `stem` is its file-name stem
/// (for the env sidecar path), `contents` already carries the managed
/// `Pod=`/`EnvironmentFile=` lines.
struct PreparedContainer {
    file_name: String,
    stem: String,
    contents: String,
    env_pairs: Vec<(String, String)>,
}

/// Validates every staged new container -- name, uniqueness (against `all`
/// and against each other), env-var syntax, and the resulting file's
/// structural validity (`writer::validate`, the same dry-run
/// `core::create_unit` runs internally) -- entirely before any write.
/// Unlike attaching an *already-existing* container (a no-op to retry on
/// failure), a rejected brand-new container has nothing else backing it up:
/// without this upfront pass, a write-time failure would silently discard
/// whatever the user just typed into that container's editor.
fn prepare_new_containers(
    state: &AppState,
    all: &[QuadletUnit],
    pod_file_name: &str,
    drafts: &[NewContainerDraft],
) -> Result<Vec<PreparedContainer>, String> {
    let mut prepared = Vec::with_capacity(drafts.len());
    let mut seen: Vec<String> = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let name = draft.name.trim();
        let file_name =
            naming::compose_file_name(name, UnitKind::Container).map_err(|e| e.to_string())?;
        if seen.contains(&file_name) || all.iter().any(|u| u.file_name == file_name) {
            return Err(format!("{file_name} already exists"));
        }
        let pairs =
            envfile::parse_editor_lines(&draft.env).map_err(|msg| format!("{file_name}: {msg}"))?;

        let contents = containerref::set_pod(&draft.contents, Some(pod_file_name));
        let contents = if pairs.is_empty() {
            contents
        } else {
            let refval = envfile::reference_value(&state.quadlet_dir, name);
            envfile::patch_environment_file(&contents, "Container", &refval, true)
        };
        writer::validate(&file_name, &contents).map_err(|e| format!("{file_name}: {e}"))?;

        seen.push(file_name.clone());
        prepared.push(PreparedContainer {
            file_name,
            stem: name.to_string(),
            contents,
            env_pairs: pairs,
        });
    }
    Ok(prepared)
}

/// Writes each pre-validated new container: the env sidecar first (if any
/// pairs), then the quadlet file, rolling the sidecar back if the file write
/// then fails and the sidecar didn't already exist -- mirrors
/// `raw_create::create`'s rollback for the same situation. Everything here
/// was already structurally validated by `prepare_new_containers`, so a
/// failure at this point is a genuine race (e.g. a file appearing
/// concurrently); it's logged and skipped rather than unwinding the pod (and
/// any already-created containers) already written and broadcast by this
/// point -- the same residual-risk class the existing-container-attach loop
/// below already accepts.
async fn create_new_containers(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    group: &str,
    prepared: &[PreparedContainer],
) {
    for c in prepared {
        let sidecar_existed = envfile::path_for(&state.quadlet_dir, &c.stem).exists();
        if !c.env_pairs.is_empty()
            && let Err(e) = envfile::save(&state.quadlet_dir, &c.stem, &c.env_pairs)
        {
            tracing::warn!(file = c.file_name, error = %e, "failed to write env sidecar for new container");
            continue;
        }
        if let Err(e) =
            core::create_unit(state, session, csrf_token, &c.file_name, group, &c.contents).await
        {
            tracing::warn!(file = c.file_name, error = %e, "failed to create new container for pod");
            if !c.env_pairs.is_empty() && !sidecar_existed {
                let _ = envfile::delete(&state.quadlet_dir, &c.stem);
            }
        }
    }
}

/// Builds the template's row list from staged drafts -- used on both a
/// fresh page (empty) and a 422 redisplay (populated), see [`Staged`].
fn new_container_rows(drafts: &[NewContainerDraft]) -> Vec<NewContainerRow<'_>> {
    drafts
        .iter()
        .map(|d| NewContainerRow {
            id: d.id.to_string(),
            name_prefill: &d.name,
            contents_prefill: &d.contents,
            env_prefill: &d.env,
        })
        .collect()
}

/// One current pod member's submitted raw contents + env text, from the
/// `memberc_{file_name}_contents`/`_env` fields. Unlike [`NewContainerDraft`]
/// there's no `id` indirection: `members` (see [`parse_member_drafts`]) is
/// itself the authoritative, server-computed list of which containers exist
/// to edit, so each is read directly by its already-existing file name.
struct MemberDraft {
    file_name: String,
    contents: String,
    env: String,
}

/// Reads one `MemberDraft` per entry in `members` (the pod's current
/// membership, from `refs::pod_members`) out of the submitted form.
fn parse_member_drafts(fields: &HashMap<String, String>, members: &[String]) -> Vec<MemberDraft> {
    members
        .iter()
        .map(|file_name| MemberDraft {
            file_name: file_name.clone(),
            contents: fields
                .get(&templates::pods::member_contents_field(file_name))
                .cloned()
                .unwrap_or_default(),
            env: fields
                .get(&templates::pods::member_env_field(file_name))
                .cloned()
                .unwrap_or_default(),
        })
        .collect()
}

/// A validated, write-ready member-container edit -- `stem` is the file-name
/// stem (for the env sidecar path), `contents` already carries the managed
/// `EnvironmentFile=` line. No name/uniqueness check, unlike
/// [`PreparedContainer`]: the file already exists and isn't being renamed.
struct PreparedMemberEdit {
    file_name: String,
    stem: String,
    contents: String,
    env_pairs: Vec<(String, String)>,
}

/// Validates every staged member edit -- env-var syntax and the resulting
/// file's structural validity -- entirely before any write, same reasoning
/// as [`prepare_new_containers`]. The error carries the failing member's
/// file name separately from its message (rather than one pre-formatted
/// string, like every other `prepare_*` here) so the caller can force that
/// one row's `<details>` open on redisplay -- it's the one collapsed
/// section a rejected submission needs the user to actually see.
fn prepare_member_edits(
    state: &AppState,
    drafts: &[MemberDraft],
) -> Result<Vec<PreparedMemberEdit>, (String, String)> {
    let mut prepared = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let stem = naming::stem(&draft.file_name).to_string();
        let pairs = envfile::parse_editor_lines(&draft.env)
            .map_err(|msg| (draft.file_name.clone(), msg))?;
        let refval = envfile::reference_value(&state.quadlet_dir, &stem);
        let contents = envfile::patch_environment_file(
            &draft.contents,
            "Container",
            &refval,
            !pairs.is_empty(),
        );
        writer::validate(&draft.file_name, &contents)
            .map_err(|e| (draft.file_name.clone(), e.to_string()))?;
        prepared.push(PreparedMemberEdit {
            file_name: draft.file_name.clone(),
            stem,
            contents,
            env_pairs: pairs,
        });
    }
    Ok(prepared)
}

/// Writes each pre-validated member edit: the env sidecar first (capturing
/// its prior value for rollback), then the quadlet file itself, restoring
/// the sidecar if that write then fails -- same shape as
/// `edit_delete::edit_submit`'s Container path, and the same residual-risk
/// tradeoff as [`create_new_containers`] (logged and skipped, not unwound,
/// on a genuine write-time race).
async fn apply_member_edits(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    prepared: &[PreparedMemberEdit],
) {
    for m in prepared {
        let prev_env = envfile::load(&state.quadlet_dir, &m.stem)
            .ok()
            .filter(|v| !v.is_empty());
        if let Err(e) = envfile::save(&state.quadlet_dir, &m.stem, &m.env_pairs) {
            tracing::warn!(file = m.file_name, error = %e, "failed to write env sidecar for pod member edit");
            continue;
        }
        if let Err(e) = core::edit_unit(state, session, csrf_token, &m.file_name, &m.contents).await
        {
            tracing::warn!(file = m.file_name, error = %e, "failed to save pod member edit");
            let res = match prev_env {
                Some(vars) => envfile::save(&state.quadlet_dir, &m.stem, &vars),
                None => envfile::delete(&state.quadlet_dir, &m.stem),
            };
            if let Err(e) = res {
                tracing::warn!(stem = m.stem, error = %e, "failed to roll back env sidecar");
            }
        }
    }
}

/// One brand-new network staged via the pod page's "New networks" field --
/// see [`NewContainerDraft`]; simpler (no env vars, no `Pod=` patching -- a
/// network doesn't reference the pod itself, the pod's own `[Pod]` section
/// gets a `Network=` line pointing at it instead, same as an *existing*
/// network attach, see [`apply_pod_resources`]), and keyed by `newnet_{id}_*`.
struct NewNetworkDraft {
    id: u32,
    name: String,
    contents: String,
}

/// Sanity cap on new-network rows per submission -- see [`MAX_NEW_CONTAINERS`].
const MAX_NEW_NETWORKS: usize = 20;

fn parse_new_networks(fields: &HashMap<String, String>) -> Vec<NewNetworkDraft> {
    let mut ids: Vec<u32> = fields
        .keys()
        .filter_map(|k| {
            k.strip_prefix("newnet_")?
                .strip_suffix("_name")?
                .parse()
                .ok()
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.truncate(MAX_NEW_NETWORKS);

    ids.into_iter()
        .map(|id| NewNetworkDraft {
            id,
            name: fields
                .get(&format!("newnet_{id}_name"))
                .cloned()
                .unwrap_or_default(),
            contents: fields
                .get(&format!("newnet_{id}_contents"))
                .cloned()
                .unwrap_or_default(),
        })
        .collect()
}

/// A validated, write-ready new network or volume -- no env vars, no `Pod=`
/// patching, unlike [`PreparedContainer`]; shared by networks and (plus a
/// mount-path destination) volumes.
#[derive(Debug)]
struct PreparedResource {
    file_name: String,
    contents: String,
}

/// Validates every staged new network -- name, uniqueness (against `all` and
/// against each other), and the resulting file's structural validity --
/// entirely before any write. See [`prepare_new_containers`].
fn prepare_new_networks(
    all: &[QuadletUnit],
    drafts: &[NewNetworkDraft],
) -> Result<Vec<PreparedResource>, String> {
    let mut prepared = Vec::with_capacity(drafts.len());
    let mut seen: Vec<String> = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let name = draft.name.trim();
        let file_name =
            naming::compose_file_name(name, UnitKind::Network).map_err(|e| e.to_string())?;
        if seen.contains(&file_name) || all.iter().any(|u| u.file_name == file_name) {
            return Err(format!("{file_name} already exists"));
        }
        writer::validate(&file_name, &draft.contents).map_err(|e| format!("{file_name}: {e}"))?;
        seen.push(file_name.clone());
        prepared.push(PreparedResource {
            file_name,
            contents: draft.contents.clone(),
        });
    }
    Ok(prepared)
}

/// Writes each pre-validated new network -- see
/// [`create_new_containers`]'s doc comment for the same residual-risk
/// tradeoff (logged and skipped, not unwound, on a genuine write-time race).
async fn create_new_networks(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    group: &str,
    prepared: &[PreparedResource],
) {
    for r in prepared {
        if let Err(e) =
            core::create_unit(state, session, csrf_token, &r.file_name, group, &r.contents).await
        {
            tracing::warn!(file = r.file_name, error = %e, "failed to create new network for pod");
        }
    }
}

fn new_network_rows(drafts: &[NewNetworkDraft]) -> Vec<NewNetworkRow<'_>> {
    drafts
        .iter()
        .map(|d| NewNetworkRow {
            id: d.id.to_string(),
            name_prefill: &d.name,
            contents_prefill: &d.contents,
        })
        .collect()
}

/// One brand-new volume staged via the pod page's "New volumes" field --
/// same shape as [`NewNetworkDraft`] plus `dest`, the mount path to give it
/// inside the pod's containers (same role as the existing-volume picker's
/// per-row destination input, see [`parse_volume_lines`]).
struct NewVolumeDraft {
    id: u32,
    name: String,
    contents: String,
    dest: String,
}

/// Sanity cap on new-volume rows per submission -- see [`MAX_NEW_CONTAINERS`].
const MAX_NEW_VOLUMES: usize = 20;

fn parse_new_volumes(fields: &HashMap<String, String>) -> Vec<NewVolumeDraft> {
    let mut ids: Vec<u32> = fields
        .keys()
        .filter_map(|k| {
            k.strip_prefix("newvol_")?
                .strip_suffix("_name")?
                .parse()
                .ok()
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.truncate(MAX_NEW_VOLUMES);

    ids.into_iter()
        .map(|id| NewVolumeDraft {
            id,
            name: fields
                .get(&format!("newvol_{id}_name"))
                .cloned()
                .unwrap_or_default(),
            contents: fields
                .get(&format!("newvol_{id}_contents"))
                .cloned()
                .unwrap_or_default(),
            dest: fields
                .get(&format!("newvol_{id}_dest"))
                .cloned()
                .unwrap_or_default(),
        })
        .collect()
}

/// A validated, write-ready new volume -- `dest` is folded into the pod's
/// own `[Pod]` section as `Volume=<file_name>:<dest>` by
/// [`apply_pod_resources`], same as an already-existing volume attach.
#[derive(Debug)]
struct PreparedVolume {
    file_name: String,
    contents: String,
    dest: String,
}

/// Validates every staged new volume -- name, uniqueness, mount-path shape
/// (see [`parse_volume_lines`]), and structural validity -- entirely before
/// any write. See [`prepare_new_containers`].
fn prepare_new_volumes(
    all: &[QuadletUnit],
    drafts: &[NewVolumeDraft],
) -> Result<Vec<PreparedVolume>, String> {
    let mut prepared = Vec::with_capacity(drafts.len());
    let mut seen: Vec<String> = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let name = draft.name.trim();
        let file_name =
            naming::compose_file_name(name, UnitKind::Volume).map_err(|e| e.to_string())?;
        if seen.contains(&file_name) || all.iter().any(|u| u.file_name == file_name) {
            return Err(format!("{file_name} already exists"));
        }
        let dest = draft.dest.trim();
        if dest.is_empty() {
            return Err(format!("{file_name} needs a mount path"));
        }
        if dest.contains([':', '\n', '\r']) {
            return Err(format!(
                "{file_name}: mount path can't contain ':' or a newline"
            ));
        }
        writer::validate(&file_name, &draft.contents).map_err(|e| format!("{file_name}: {e}"))?;
        seen.push(file_name.clone());
        prepared.push(PreparedVolume {
            file_name,
            contents: draft.contents.clone(),
            dest: dest.to_string(),
        });
    }
    Ok(prepared)
}

/// Writes each pre-validated new volume -- see [`create_new_networks`].
async fn create_new_volumes(
    state: &AppState,
    session: &Session,
    csrf_token: &str,
    group: &str,
    prepared: &[PreparedVolume],
) {
    for r in prepared {
        if let Err(e) =
            core::create_unit(state, session, csrf_token, &r.file_name, group, &r.contents).await
        {
            tracing::warn!(file = r.file_name, error = %e, "failed to create new volume for pod");
        }
    }
}

fn new_volume_rows(drafts: &[NewVolumeDraft]) -> Vec<NewVolumeRow<'_>> {
    drafts
        .iter()
        .map(|d| NewVolumeRow {
            id: d.id.to_string(),
            name_prefill: &d.name,
            contents_prefill: &d.contents,
            dest_prefill: &d.dest,
        })
        .collect()
}

/// Everything staged for the container/network/volume pickers and the
/// "New containers" field, carried through a rejected submission's
/// redisplay so nothing already picked or typed is lost. Defaults to
/// all-empty for a fresh page load.
#[derive(Default)]
struct Staged<'a> {
    existing_containers: &'a str,
    restart_checked: bool,
    networks: &'a str,
    volumes: &'a str,
    new_containers: &'a [NewContainerDraft],
    new_networks: &'a [NewNetworkDraft],
    new_volumes: &'a [NewVolumeDraft],
}

pub async fn new_form(State(state): State<AppState>, session: Session) -> impl IntoResponse {
    let csrf = crate::auth::csrf::current(&session)
        .await
        .unwrap_or_default();
    let host_vars = crate::hostenv::load().unwrap_or_default();
    let known_groups = discovery::list_groups(&state.quadlet_dir);
    let all = discovery::load_all(&state.quadlet_dir).unwrap_or_default();
    let available_containers = container_options(&all, None);
    let available_networks = resource_options(&all, UnitKind::Network, &[]);
    let available_volumes = resource_options(&all, UnitKind::Volume, &[]);

    templates::pods::new_page(NewPodPage {
        csrf: &csrf,
        stem_prefill: "",
        group_prefill: "",
        known_groups: &known_groups,
        contents_body: "[Pod]\n",
        existing_containers_body: "",
        available_containers: &available_containers,
        restart_checked: false,
        networks_body: "",
        available_networks: &available_networks,
        volumes_body: "",
        available_volumes: &available_volumes,
        new_containers: &[],
        new_networks: &[],
        new_volumes: &[],
        host_vars: &host_vars,
        error: None,
        health: state.health.get(),
    })
}

pub async fn create(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, PageError> {
    let csrf_token = form.get("csrf_token").cloned().unwrap_or_default();
    if !crate::auth::csrf::verify(&session, &csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    let stem = form
        .get("file_name")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let group = form
        .get("group")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let contents = form.get("contents").cloned().unwrap_or_default();
    let existing_body = form
        .get("existing_containers")
        .map(String::as_str)
        .unwrap_or("");
    let networks_body = form.get("pod_networks").map(String::as_str).unwrap_or("");
    let volumes_body = form.get("pod_volumes").map(String::as_str).unwrap_or("");
    let restart_checked = form.contains_key("restart_containers");
    let new_container_drafts = parse_new_containers(&form);
    let new_network_drafts = parse_new_networks(&form);
    let new_volume_drafts = parse_new_volumes(&form);

    let staged = Staged {
        existing_containers: existing_body,
        restart_checked,
        networks: networks_body,
        volumes: volumes_body,
        new_containers: &new_container_drafts,
        new_networks: &new_network_drafts,
        new_volumes: &new_volume_drafts,
    };
    let existing = parse_existing(existing_body);
    let networks = parse_existing(networks_body);

    macro_rules! reject {
        ($msg:expr) => {{
            return Ok(redisplay(&state, &session, &stem, &group, &contents, &staged, &$msg).await);
        }};
    }

    let volumes = match parse_volume_lines(volumes_body) {
        Ok(v) => v,
        Err(msg) => reject!(msg),
    };

    let Ok(pod_file_name) = naming::compose_file_name(&stem, UnitKind::Pod) else {
        reject!(format!(
            "invalid file name '{stem}': use letters, digits, '_', '-', '.', '@'; no '/' or '..'"
        ));
    };
    if !naming::valid_group(&group) {
        reject!(format!("invalid group '{group}'"));
    }

    // Validate every attach list, and every new container, up front so pod
    // creation stays effectively all-or-nothing for every failure a normal
    // submission can hit; only a genuine race between this check and the
    // writes below (rare, and no worse than `create_unit`'s own
    // existence-check/write race) can still leave a partial result.
    let all = discovery::load_all(&state.quadlet_dir).unwrap_or_default();
    if let Err(msg) = validate_existing(&all, &existing) {
        reject!(msg);
    }
    if let Err(msg) = validate_networks(&all, &networks) {
        reject!(msg);
    }
    if let Err(msg) = validate_volumes(&all, &volumes) {
        reject!(msg);
    }
    let prepared_containers =
        match prepare_new_containers(&state, &all, &pod_file_name, &new_container_drafts) {
            Ok(p) => p,
            Err(msg) => reject!(msg),
        };
    let prepared_networks = match prepare_new_networks(&all, &new_network_drafts) {
        Ok(p) => p,
        Err(msg) => reject!(msg),
    };
    let prepared_volumes = match prepare_new_volumes(&all, &new_volume_drafts) {
        Ok(p) => p,
        Err(msg) => reject!(msg),
    };

    // Brand-new networks/volumes are folded in alongside already-existing
    // attaches -- `apply_pod_resources` doesn't care which is which, it just
    // needs every file name up front to patch the pod's own `[Pod]` section.
    let mut all_networks = networks.clone();
    all_networks.extend(prepared_networks.iter().map(|p| p.file_name.clone()));
    let mut all_volumes = volumes.clone();
    all_volumes.extend(
        prepared_volumes
            .iter()
            .map(|p| (p.file_name.clone(), p.dest.clone())),
    );

    let final_contents = apply_pod_resources(&contents, &all_networks, &all_volumes);

    match core::create_unit(
        &state,
        &session,
        &csrf_token,
        &pod_file_name,
        &group,
        &final_contents,
    )
    .await
    {
        Ok(unit) => {
            for file_name in &existing {
                if let Err(e) = core::attach_container_to_pod(
                    &state,
                    file_name,
                    &pod_file_name,
                    restart_checked,
                )
                .await
                {
                    tracing::warn!(file = file_name, pod = %pod_file_name, error = %e, "failed to attach existing container to new pod");
                }
            }
            create_new_networks(&state, &session, &csrf_token, &group, &prepared_networks).await;
            create_new_volumes(&state, &session, &csrf_token, &group, &prepared_volumes).await;
            create_new_containers(&state, &session, &csrf_token, &group, &prepared_containers)
                .await;
            Ok(Redirect::to(&core::unit_url(&unit)).into_response())
        }
        Err(AppError::Quadlet(e)) if e.is_client_error() => Ok(redisplay(
            &state,
            &session,
            &stem,
            &group,
            &contents,
            &staged,
            &e.to_string(),
        )
        .await),
        Err(e) => Err(e.into()),
    }
}

async fn redisplay(
    state: &AppState,
    session: &Session,
    stem: &str,
    group: &str,
    contents: &str,
    staged: &Staged<'_>,
    error: &str,
) -> Response {
    let csrf = crate::auth::csrf::current(session)
        .await
        .unwrap_or_default();
    let host_vars = crate::hostenv::load().unwrap_or_default();
    let known_groups = discovery::list_groups(&state.quadlet_dir);
    let all = discovery::load_all(&state.quadlet_dir).unwrap_or_default();
    let available_containers = container_options(&all, None);
    let available_networks = resource_options(&all, UnitKind::Network, &[]);
    let available_volumes = resource_options(&all, UnitKind::Volume, &[]);
    let new_containers = new_container_rows(staged.new_containers);
    let new_networks = new_network_rows(staged.new_networks);
    let new_volumes = new_volume_rows(staged.new_volumes);
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        templates::pods::new_page(NewPodPage {
            csrf: &csrf,
            stem_prefill: stem,
            group_prefill: group,
            known_groups: &known_groups,
            contents_body: contents,
            existing_containers_body: staged.existing_containers,
            available_containers: &available_containers,
            restart_checked: staged.restart_checked,
            networks_body: staged.networks,
            available_networks: &available_networks,
            volumes_body: staged.volumes,
            available_volumes: &available_volumes,
            new_containers: &new_containers,
            new_networks: &new_networks,
            new_volumes: &new_volumes,
            host_vars: &host_vars,
            error: Some(error),
            health: state.health.get(),
        }),
    )
        .into_response()
}

pub async fn edit_form(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
) -> Result<Response, PageError> {
    render_edit(
        &state,
        &session,
        &file_name,
        None,
        &Staged::default(),
        None,
        None,
        None,
    )
    .await
}

pub async fn edit_submit(
    State(state): State<AppState>,
    session: Session,
    Path(file_name): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Response, PageError> {
    let csrf_token = form.get("csrf_token").cloned().unwrap_or_default();
    if !crate::auth::csrf::verify(&session, &csrf_token).await {
        return Err(AppError::Csrf.into());
    }

    let contents = form.get("contents").cloned().unwrap_or_default();
    let existing_body = form
        .get("existing_containers")
        .map(String::as_str)
        .unwrap_or("");
    let networks_body = form.get("pod_networks").map(String::as_str).unwrap_or("");
    let volumes_body = form.get("pod_volumes").map(String::as_str).unwrap_or("");
    let restart_checked = form.contains_key("restart_containers");
    let new_container_drafts = parse_new_containers(&form);
    let new_network_drafts = parse_new_networks(&form);
    let new_volume_drafts = parse_new_volumes(&form);

    let staged = Staged {
        existing_containers: existing_body,
        restart_checked,
        networks: networks_body,
        volumes: volumes_body,
        new_containers: &new_container_drafts,
        new_networks: &new_network_drafts,
        new_volumes: &new_volume_drafts,
    };
    let existing = parse_existing(existing_body);
    let networks = parse_existing(networks_body);
    let volumes = match parse_volume_lines(volumes_body) {
        Ok(v) => v,
        Err(msg) => {
            return render_edit(
                &state,
                &session,
                &file_name,
                Some(&contents),
                &staged,
                Some(&form),
                None,
                Some(&msg),
            )
            .await;
        }
    };

    let all = discovery::load_all(&state.quadlet_dir).unwrap_or_default();
    let pod_unit = all.iter().find(|u| u.file_name == file_name);
    // Newly-defined containers are filed alongside their pod.
    let pod_group = pod_unit.map(|u| u.group.clone()).unwrap_or_default();
    let members = pod_unit
        .map(|u| refs::pod_members(u, &all))
        .unwrap_or_default();
    let member_drafts = parse_member_drafts(&form, &members);

    for check in [
        validate_existing(&all, &existing),
        validate_networks(&all, &networks),
        validate_volumes(&all, &volumes),
    ] {
        if let Err(msg) = check {
            return render_edit(
                &state,
                &session,
                &file_name,
                Some(&contents),
                &staged,
                Some(&form),
                None,
                Some(&msg),
            )
            .await;
        }
    }
    let prepared_containers =
        match prepare_new_containers(&state, &all, &file_name, &new_container_drafts) {
            Ok(p) => p,
            Err(msg) => {
                return render_edit(
                    &state,
                    &session,
                    &file_name,
                    Some(&contents),
                    &staged,
                    Some(&form),
                    None,
                    Some(&msg),
                )
                .await;
            }
        };
    let prepared_networks = match prepare_new_networks(&all, &new_network_drafts) {
        Ok(p) => p,
        Err(msg) => {
            return render_edit(
                &state,
                &session,
                &file_name,
                Some(&contents),
                &staged,
                Some(&form),
                None,
                Some(&msg),
            )
            .await;
        }
    };
    let prepared_volumes = match prepare_new_volumes(&all, &new_volume_drafts) {
        Ok(p) => p,
        Err(msg) => {
            return render_edit(
                &state,
                &session,
                &file_name,
                Some(&contents),
                &staged,
                Some(&form),
                None,
                Some(&msg),
            )
            .await;
        }
    };
    let prepared_members = match prepare_member_edits(&state, &member_drafts) {
        Ok(p) => p,
        Err((failed_file, msg)) => {
            return render_edit(
                &state,
                &session,
                &file_name,
                Some(&contents),
                &staged,
                Some(&form),
                Some(&failed_file),
                Some(&format!("{failed_file}: {msg}")),
            )
            .await;
        }
    };

    let mut all_networks = networks.clone();
    all_networks.extend(prepared_networks.iter().map(|p| p.file_name.clone()));
    let mut all_volumes = volumes.clone();
    all_volumes.extend(
        prepared_volumes
            .iter()
            .map(|p| (p.file_name.clone(), p.dest.clone())),
    );

    let final_contents = apply_pod_resources(&contents, &all_networks, &all_volumes);

    match core::edit_unit(&state, &session, &csrf_token, &file_name, &final_contents).await {
        Ok(unit) => {
            for f in &existing {
                if let Err(e) =
                    core::attach_container_to_pod(&state, f, &file_name, restart_checked).await
                {
                    tracing::warn!(file = f, pod = %file_name, error = %e, "failed to attach existing container to pod");
                }
            }
            apply_member_edits(&state, &session, &csrf_token, &prepared_members).await;
            create_new_networks(
                &state,
                &session,
                &csrf_token,
                &pod_group,
                &prepared_networks,
            )
            .await;
            create_new_volumes(&state, &session, &csrf_token, &pod_group, &prepared_volumes).await;
            create_new_containers(
                &state,
                &session,
                &csrf_token,
                &pod_group,
                &prepared_containers,
            )
            .await;
            Ok(Redirect::to(&core::unit_url(&unit)).into_response())
        }
        Err(AppError::Quadlet(e)) if e.is_client_error() => {
            render_edit(
                &state,
                &session,
                &file_name,
                Some(&contents),
                &staged,
                Some(&form),
                None,
                Some(&e.to_string()),
            )
            .await
        }
        Err(e) => Err(e.into()),
    }
}

/// Builds the Members section's row list -- every container currently in
/// the pod, each pre-filled from `form_for_members` (a rejected submission's
/// posted values, so an edit isn't lost on a 422) or, failing that, loaded
/// fresh from disk/the env sidecar, exactly as `edit_delete::render_edit`
/// loads a standalone container edit page.
fn member_rows(
    state: &AppState,
    all: &[QuadletUnit],
    members: &[String],
    form_for_members: Option<&HashMap<String, String>>,
    open_member: Option<&str>,
) -> Vec<templates::pods::MemberContainerRow> {
    members
        .iter()
        .filter_map(|file_name| {
            let member = all.iter().find(|u| &u.file_name == file_name)?;
            let contents = form_for_members
                .and_then(|f| f.get(&templates::pods::member_contents_field(file_name)))
                .cloned()
                .unwrap_or_else(|| member.raw.clone());
            let env = match form_for_members
                .and_then(|f| f.get(&templates::pods::member_env_field(file_name)))
            {
                Some(s) => s.clone(),
                None => envfile::to_editor_lines(
                    &envfile::load(&state.quadlet_dir, naming::stem(file_name)).unwrap_or_default(),
                ),
            };
            Some(templates::pods::MemberContainerRow {
                open: open_member == Some(file_name.as_str()),
                file_name: file_name.clone(),
                group: member.group.clone(),
                contents_prefill: contents,
                env_prefill: env,
            })
        })
        .collect()
}

/// Renders the Edit Pod form. `contents_override` supplies the editor body
/// on a 422 redisplay (the text as submitted); otherwise it's loaded fresh
/// from disk. `form_for_members` is the full submitted form on a 422
/// redisplay (so member-row edits aren't lost, see [`member_rows`]); `None`
/// on a fresh GET. `open_member` is the file name of the one member row (if
/// any) to force open -- set only when that member's own edit is what got
/// this redisplay rejected, so a collapsed row never hides its own error.
#[allow(clippy::too_many_arguments)]
async fn render_edit(
    state: &AppState,
    session: &Session,
    file_name: &str,
    contents_override: Option<&str>,
    staged: &Staged<'_>,
    form_for_members: Option<&HashMap<String, String>>,
    open_member: Option<&str>,
    error: Option<&str>,
) -> Result<Response, PageError> {
    let unit = discovery::load_by_name(&state.quadlet_dir, file_name)?;
    let csrf = crate::auth::csrf::current(session)
        .await
        .unwrap_or_default();
    let host_vars = crate::hostenv::load().unwrap_or_default();
    let all = discovery::load_all(&state.quadlet_dir).unwrap_or_default();
    let (current_networks, current_volumes) = refs::pod_own_refs(&unit, &all);
    let available_containers = container_options(&all, Some(file_name));
    let available_networks = resource_options(&all, UnitKind::Network, &current_networks);
    let available_volumes = resource_options(&all, UnitKind::Volume, &current_volumes);
    let members = member_rows(
        state,
        &all,
        &refs::pod_members(&unit, &all),
        form_for_members,
        open_member,
    );
    let new_containers = new_container_rows(staged.new_containers);
    let new_networks = new_network_rows(staged.new_networks);
    let new_volumes = new_volume_rows(staged.new_volumes);
    let contents = contents_override.unwrap_or(&unit.raw);
    let status = if error.is_some() {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        templates::pods::edit_page(EditPodPage {
            csrf: &csrf,
            base_url: &core::unit_url(&unit),
            file_name: &unit.file_name,
            members: &members,
            contents_body: contents,
            existing_containers_body: staged.existing_containers,
            available_containers: &available_containers,
            restart_checked: staged.restart_checked,
            networks_body: staged.networks,
            available_networks: &available_networks,
            volumes_body: staged.volumes,
            available_volumes: &available_volumes,
            new_containers: &new_containers,
            new_networks: &new_networks,
            new_volumes: &new_volumes,
            host_vars: &host_vars,
            error,
            health: state.health.get(),
        }),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quadlet::model::Section;

    fn unit(
        file_name: &str,
        kind: UnitKind,
        section: &str,
        entries: &[(&str, &str)],
    ) -> QuadletUnit {
        QuadletUnit {
            file_name: file_name.into(),
            group: String::new(),
            path: format!("/tmp/{file_name}").into(),
            kind,
            sections: vec![Section {
                name: section.into(),
                entries: entries
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }],
            raw: String::new(),
        }
    }

    fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn network_draft(id: u32, name: &str, contents: &str) -> NewNetworkDraft {
        NewNetworkDraft {
            id,
            name: name.to_string(),
            contents: contents.to_string(),
        }
    }

    fn volume_draft(id: u32, name: &str, contents: &str, dest: &str) -> NewVolumeDraft {
        NewVolumeDraft {
            id,
            name: name.to_string(),
            contents: contents.to_string(),
            dest: dest.to_string(),
        }
    }

    // -- parse_existing --------------------------------------------------

    #[test]
    fn parse_existing_trims_and_skips_blank_lines() {
        let out = parse_existing("  a.container  \n\nb.container\n   \n");
        assert_eq!(out, vec!["a.container", "b.container"]);
    }

    #[test]
    fn parse_existing_of_blank_input_is_empty() {
        assert!(parse_existing("").is_empty());
        assert!(parse_existing("   \n  \n").is_empty());
    }

    // -- parse_volume_lines ------------------------------------------------

    #[test]
    fn parse_volume_lines_parses_file_dest_pairs() {
        let out = parse_volume_lines("data.volume=/data\ncache.volume=/cache\n").unwrap();
        assert_eq!(
            out,
            vec![
                ("data.volume".to_string(), "/data".to_string()),
                ("cache.volume".to_string(), "/cache".to_string()),
            ]
        );
    }

    #[test]
    fn parse_volume_lines_skips_blank_lines() {
        let out = parse_volume_lines("\n data.volume=/data \n\n").unwrap();
        assert_eq!(out, vec![("data.volume".to_string(), "/data".to_string())]);
    }

    #[test]
    fn parse_volume_lines_rejects_a_line_with_no_equals() {
        let err = parse_volume_lines("data.volume").unwrap_err();
        assert_eq!(err, "line 1: expected FILE=DEST");
    }

    #[test]
    fn parse_volume_lines_rejects_an_empty_mount_path() {
        let err = parse_volume_lines("data.volume=").unwrap_err();
        assert_eq!(err, "line 1: 'data.volume' needs a mount path");
    }

    #[test]
    fn parse_volume_lines_rejects_a_colon_in_the_mount_path() {
        let err = parse_volume_lines("data.volume=/data:ro").unwrap_err();
        assert_eq!(err, "line 1: mount path can't contain ':' or a newline");
    }

    #[test]
    fn parse_volume_lines_error_uses_a_one_based_line_number() {
        let err = parse_volume_lines("data.volume=/data\nbad-line\n").unwrap_err();
        assert_eq!(err, "line 2: expected FILE=DEST");
    }

    // -- validate_existing / validate_networks / validate_volumes ----------

    #[test]
    fn validate_existing_accepts_a_real_container() {
        let all = vec![unit("web.container", UnitKind::Container, "Container", &[])];
        assert!(validate_existing(&all, &["web.container".to_string()]).is_ok());
    }

    #[test]
    fn validate_existing_rejects_a_missing_file() {
        let err = validate_existing(&[], &["ghost.container".to_string()]).unwrap_err();
        assert_eq!(err, "ghost.container not found");
    }

    #[test]
    fn validate_existing_rejects_the_wrong_kind() {
        let all = vec![unit("data.volume", UnitKind::Volume, "Volume", &[])];
        let err = validate_existing(&all, &["data.volume".to_string()]).unwrap_err();
        assert_eq!(err, "data.volume is not a container");
    }

    #[test]
    fn validate_networks_rejects_the_wrong_kind() {
        let all = vec![unit("web.container", UnitKind::Container, "Container", &[])];
        let err = validate_networks(&all, &["web.container".to_string()]).unwrap_err();
        assert_eq!(err, "web.container is not a network");
    }

    #[test]
    fn validate_volumes_rejects_the_wrong_kind() {
        let all = vec![unit("frontend.network", UnitKind::Network, "Network", &[])];
        let err = validate_volumes(
            &all,
            &[("frontend.network".to_string(), "/data".to_string())],
        )
        .unwrap_err();
        assert_eq!(err, "frontend.network is not a volume");
    }

    // -- apply_pod_resources -------------------------------------------------

    #[test]
    fn apply_pod_resources_adds_network_and_volume_lines() {
        let out = apply_pod_resources(
            "[Pod]\n",
            &["frontend.network".to_string()],
            &[("data.volume".to_string(), "/data".to_string())],
        );
        assert_eq!(
            out,
            "[Pod]\nNetwork=frontend.network\nVolume=data.volume:/data\n"
        );
    }

    #[test]
    fn apply_pod_resources_with_nothing_staged_is_unchanged() {
        assert_eq!(apply_pod_resources("[Pod]\n", &[], &[]), "[Pod]\n");
    }

    // -- parse_member_drafts ------------------------------------------------

    #[test]
    fn parse_member_drafts_reads_one_draft_per_member_by_file_name() {
        let form = fields(&[
            ("memberc_web.container_contents", "[Container]\nImage=a\n"),
            ("memberc_web.container_env", "A=1"),
            (
                "memberc_worker.container_contents",
                "[Container]\nImage=b\n",
            ),
            ("unrelated_field", "ignored"),
        ]);
        let members = vec!["web.container".to_string(), "worker.container".to_string()];
        let drafts = parse_member_drafts(&form, &members);
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].file_name, "web.container");
        assert_eq!(drafts[0].contents, "[Container]\nImage=a\n");
        assert_eq!(drafts[0].env, "A=1");
        assert_eq!(drafts[1].file_name, "worker.container");
        assert_eq!(drafts[1].contents, "[Container]\nImage=b\n");
        assert_eq!(drafts[1].env, "");
    }

    #[test]
    fn parse_member_drafts_of_no_members_is_empty() {
        let form = fields(&[("memberc_web.container_contents", "[Container]\n")]);
        assert!(parse_member_drafts(&form, &[]).is_empty());
    }

    #[test]
    fn parse_member_drafts_defaults_missing_fields_to_empty() {
        let members = vec!["ghost.container".to_string()];
        let drafts = parse_member_drafts(&HashMap::new(), &members);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].file_name, "ghost.container");
        assert_eq!(drafts[0].contents, "");
        assert_eq!(drafts[0].env, "");
    }

    // -- parse_new_containers / parse_new_networks / parse_new_volumes ------

    #[test]
    fn parse_new_containers_groups_by_id_in_sorted_order() {
        let form = fields(&[
            ("newc_2_name", "worker"),
            ("newc_2_contents", "[Container]\nImage=b\n"),
            ("newc_2_env", "B=1"),
            ("newc_1_name", "web"),
            ("newc_1_contents", "[Container]\nImage=a\n"),
            ("newc_1_env", ""),
            ("unrelated_field", "ignored"),
        ]);
        let drafts = parse_new_containers(&form);
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].id, 1);
        assert_eq!(drafts[0].name, "web");
        assert_eq!(drafts[1].id, 2);
        assert_eq!(drafts[1].env, "B=1");
    }

    #[test]
    fn parse_new_containers_ignores_keys_belonging_to_other_kinds() {
        let form = fields(&[("newnet_1_name", "frontend"), ("newvol_1_name", "data")]);
        assert!(parse_new_containers(&form).is_empty());
    }

    #[test]
    fn parse_new_containers_caps_at_the_sanity_limit() {
        let mut form = HashMap::new();
        for i in 0..(MAX_NEW_CONTAINERS + 5) {
            form.insert(format!("newc_{i}_name"), format!("c{i}"));
        }
        assert_eq!(parse_new_containers(&form).len(), MAX_NEW_CONTAINERS);
    }

    #[test]
    fn parse_new_networks_reads_name_and_contents() {
        let form = fields(&[
            ("newnet_1_name", "frontend"),
            ("newnet_1_contents", "[Network]\n"),
        ]);
        let drafts = parse_new_networks(&form);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].name, "frontend");
        assert_eq!(drafts[0].contents, "[Network]\n");
    }

    #[test]
    fn parse_new_volumes_reads_name_contents_and_dest() {
        let form = fields(&[
            ("newvol_1_name", "data"),
            ("newvol_1_contents", "[Volume]\n"),
            ("newvol_1_dest", "/data"),
        ]);
        let drafts = parse_new_volumes(&form);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].dest, "/data");
    }

    // -- prepare_new_networks / prepare_new_volumes -------------------------

    #[test]
    fn prepare_new_networks_validates_and_names_the_file() {
        let drafts = vec![network_draft(1, "frontend", "[Network]\nDriver=bridge\n")];
        let prepared = prepare_new_networks(&[], &drafts).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].file_name, "frontend.network");
        assert_eq!(prepared[0].contents, "[Network]\nDriver=bridge\n");
    }

    #[test]
    fn prepare_new_networks_rejects_a_name_that_already_exists() {
        let all = vec![unit("frontend.network", UnitKind::Network, "Network", &[])];
        let drafts = vec![network_draft(1, "frontend", "[Network]\n")];
        let err = prepare_new_networks(&all, &drafts).unwrap_err();
        assert_eq!(err, "frontend.network already exists");
    }

    #[test]
    fn prepare_new_networks_rejects_duplicate_names_within_one_submission() {
        let drafts = vec![
            network_draft(1, "frontend", "[Network]\n"),
            network_draft(2, "frontend", "[Network]\n"),
        ];
        let err = prepare_new_networks(&[], &drafts).unwrap_err();
        assert_eq!(err, "frontend.network already exists");
    }

    #[test]
    fn prepare_new_networks_rejects_an_invalid_name() {
        let drafts = vec![network_draft(1, "../escape", "[Network]\n")];
        assert!(prepare_new_networks(&[], &drafts).is_err());
    }

    #[test]
    fn prepare_new_networks_rejects_structurally_invalid_contents() {
        let drafts = vec![network_draft(1, "frontend", "Driver=bridge\n")];
        assert!(prepare_new_networks(&[], &drafts).is_err());
    }

    #[test]
    fn prepare_new_volumes_needs_a_mount_path() {
        let drafts = vec![volume_draft(1, "data", "[Volume]\n", "")];
        let err = prepare_new_volumes(&[], &drafts).unwrap_err();
        assert_eq!(err, "data.volume needs a mount path");
    }

    #[test]
    fn prepare_new_volumes_rejects_a_colon_in_the_mount_path() {
        let drafts = vec![volume_draft(1, "data", "[Volume]\n", "/data:ro")];
        let err = prepare_new_volumes(&[], &drafts).unwrap_err();
        assert_eq!(
            err,
            "data.volume: mount path can't contain ':' or a newline"
        );
    }

    #[test]
    fn prepare_new_volumes_succeeds_with_a_valid_mount_path() {
        let drafts = vec![volume_draft(1, "data", "[Volume]\n", "/data")];
        let prepared = prepare_new_volumes(&[], &drafts).unwrap();
        assert_eq!(prepared[0].file_name, "data.volume");
        assert_eq!(prepared[0].dest, "/data");
    }

    // -- container_options / resource_options -------------------------------

    #[test]
    fn container_options_excludes_templates_and_other_kinds() {
        let all = vec![
            unit(
                "web.container",
                UnitKind::Container,
                "Container",
                &[("Image", "nginx")],
            ),
            unit("worker@.container", UnitKind::Container, "Container", &[]),
            unit("data.volume", UnitKind::Volume, "Volume", &[]),
        ];
        let opts = container_options(&all, None);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].file_name, "web.container");
        assert_eq!(opts[0].image.as_deref(), Some("nginx"));
        assert_eq!(opts[0].current_pod, None);
    }

    #[test]
    fn container_options_excludes_current_members_of_the_pod_being_edited() {
        let all = vec![
            unit(
                "web.container",
                UnitKind::Container,
                "Container",
                &[("Pod", "app.pod")],
            ),
            unit(
                "worker.container",
                UnitKind::Container,
                "Container",
                &[("Pod", "other.pod")],
            ),
        ];
        let opts = container_options(&all, Some("app.pod"));
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].file_name, "worker.container");
        assert_eq!(opts[0].current_pod.as_deref(), Some("other.pod"));
    }

    #[test]
    fn resource_options_filters_by_kind_and_excludes_given_names() {
        let all = vec![
            unit("frontend.network", UnitKind::Network, "Network", &[]),
            unit("backend.network", UnitKind::Network, "Network", &[]),
            unit("data.volume", UnitKind::Volume, "Volume", &[]),
        ];
        let opts = resource_options(&all, UnitKind::Network, &["frontend.network".to_string()]);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].file_name, "backend.network");
    }
}
