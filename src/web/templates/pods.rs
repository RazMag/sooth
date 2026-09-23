use maud::{Markup, html};

use super::{
    BannerKind, EditorFileName, Icon, NavItem, back_link, banner, code_editor, code_editor_named,
    csrf_input, detail, env_var_editor_named, group_field, host_vars_panel, icon, page_header,
    shell,
};
use crate::health::Health;
use crate::hostenv::EnvVar;
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;
use crate::web::core;

/// `members`/`networks`/`volumes` are file names -- `networks`/`volumes`
/// resolved into links by `super::unit_links` right in the Overview card
/// (pods rarely have more than one or two); `members` gets its own count in
/// the card plus a modal listing the full set, since a busy pod's container
/// list would otherwise cram the aside. See `refs::pod_members` and
/// `refs::pod_own_refs` for how the handler derives all three (reversing
/// Quadlet's inverted pod-membership model for containers, reading the
/// pod's own `[Pod]` section for networks/volumes).
#[allow(clippy::too_many_arguments)]
pub fn detail_page(
    unit: &QuadletUnit,
    status: &UnitStatus,
    csrf: &str,
    all_units: &[QuadletUnit],
    members: &[String],
    networks: &[String],
    volumes: &[String],
    known_groups: &[String],
    health: Health,
) -> Markup {
    let extra = if members.is_empty() {
        None
    } else {
        Some(members_dialog(&unit.file_name, all_units, members))
    };
    detail::detail_page(
        unit,
        status,
        csrf,
        &[
            ("Containers", members_count(members)),
            ("Networks", super::unit_links(all_units, networks)),
            ("Volumes", super::unit_links(all_units, volumes)),
            ("Ports", super::ports_cell_live(unit, status)),
        ],
        extra,
        known_groups,
        health,
    )
}

/// The Overview card's "Containers" value: a bare "—" when empty, otherwise
/// a count that opens [`members_dialog`] -- the list itself lives in the
/// modal, not the card, so a pod with a dozen containers doesn't blow out
/// the aside's width.
fn members_count(members: &[String]) -> Markup {
    html! {
        @if members.is_empty() {
            span.muted { "—" }
        } @else {
            button.btn-link type="button" data-pod-members-open {
                (members.len())
                @if members.len() == 1 { " container" } @else { " containers" }
            }
        }
    }
}

/// The full member list, in a dialog opened from [`members_count`] -- sooth's
/// only other modal (the "New Pod" container picker) was tried as a
/// `<dialog>` too and dropped for landing off-center; the difference here is
/// this one carries its own explicit `margin: auto` in `frontend/styles.css`
/// instead of leaning on the browser default, which Tailwind's preflight
/// (`margin: 0` on every element) was actually the one canceling out.
fn members_dialog(pod_file_name: &str, all_units: &[QuadletUnit], members: &[String]) -> Markup {
    html! {
        dialog.pod-members-dialog data-pod-members-dialog {
            div.pod-members-header {
                h2 { "Containers in " (pod_file_name) }
                button.btn-icon type="button" data-pod-members-close aria-label="Close" {
                    (icon(Icon::X))
                }
            }
            div.pod-members-list {
                @for name in members {
                    @match all_units.iter().find(|u| &u.file_name == name) {
                        Some(u) => { a href=(core::unit_url(u)) { (name) } }
                        None => { span { (name) } }
                    }
                }
            }
        }
    }
}

/// One row the "existing container" picker offers -- every non-template
/// `.container` quadlet on disk, per `handlers::pods::container_options`.
pub struct ContainerOption {
    pub file_name: String,
    pub group: String,
    pub image: Option<String>,
    /// The pod it's already attached to, if any -- picking it re-points that
    /// `Pod=` line at the pod being created here.
    pub current_pod: Option<String>,
}

/// One row the "existing network"/"existing volume" pickers offer -- every
/// non-template `.network`/`.volume` quadlet on disk, per
/// `handlers::pods::resource_options`. Simpler than [`ContainerOption`]
/// (no image/current-pod columns -- a network or volume doesn't carry
/// either), so it's shared by both pickers.
pub struct ResourceOption {
    pub file_name: String,
    pub group: String,
}

/// Parameters for [`new_page`].
pub struct NewPodPage<'a> {
    pub csrf: &'a str,
    /// The stem field's value (`""` first render, the posted stem on 422).
    pub stem_prefill: &'a str,
    pub group_prefill: &'a str,
    pub known_groups: &'a [String],
    /// The raw editor's body -- the `[Pod]` skeleton on first render, the
    /// rejected submission on a 422 redisplay.
    pub contents_body: &'a str,
    /// Newline-separated existing-container file names staged so far (`""`
    /// unless redisplaying a rejected submission).
    pub existing_containers_body: &'a str,
    pub available_containers: &'a [ContainerOption],
    /// Whether "restart attached containers that are running" is checked --
    /// `false` on first render, the posted value on a 422 redisplay.
    pub restart_checked: bool,
    /// Newline-separated existing-network file names staged to attach.
    pub networks_body: &'a str,
    pub available_networks: &'a [ResourceOption],
    /// `FILE=DEST` lines -- existing-volume file name plus the mount path
    /// staged for it.
    pub volumes_body: &'a str,
    pub available_volumes: &'a [ResourceOption],
    /// Brand-new containers staged so far -- `&[]` on first render, rebuilt
    /// from the submission on a 422 redisplay so nothing typed in any row's
    /// editor/env vars is lost.
    pub new_containers: &'a [NewContainerRow<'a>],
    /// Brand-new networks staged so far -- see `new_containers`.
    pub new_networks: &'a [NewNetworkRow<'a>],
    /// Brand-new volumes staged so far -- see `new_containers`.
    pub new_volumes: &'a [NewVolumeRow<'a>],
    pub host_vars: &'a [EnvVar],
    pub error: Option<&'a str>,
    pub health: Health,
}

/// The "New Pod" page: file name + group (as every "New" page has), pickers
/// for attaching already-defined containers/networks/volumes in place of
/// hand-typed `Pod=`/`Network=`/`Volume=` lines, a way to define brand-new
/// containers/networks/volumes inline, and the raw INI editor every "New"
/// page shows for anything the pickers don't cover.
pub fn new_page(p: NewPodPage<'_>) -> Markup {
    let file_name = EditorFileName::StemSuffix {
        input: "#file_name",
        suffix: ".pod",
    };
    let body = html! {
        (back_link("/", "Services"))
        (page_header("New pod", html! {}))
        @if let Some(msg) = p.error { (banner(BannerKind::Error, msg)) }
        form method="post" action="/pods" {
            (csrf_input(p.csrf))
            div.field {
                label for="file_name" { "File name" }
                div.stem-row {
                    input.input type="text" id="file_name" name="file_name"
                        value=(p.stem_prefill) placeholder="myapp"
                        autocomplete="off" autocapitalize="off" spellcheck="false" required;
                    span.stem-suffix { ".pod" }
                }
            }
            (group_field(p.group_prefill, p.known_groups, false))
            div.host-vars-flyout { (host_vars_panel(p.host_vars)) }
            div.pod-sections {
                (resource_card("Containers",
                    containers_field(p.available_containers, p.existing_containers_body, p.restart_checked),
                    new_containers_field(p.new_containers)))
                (resource_card("Networks",
                    networks_field(p.available_networks, p.networks_body),
                    new_networks_field(p.new_networks)))
                (resource_card("Volumes",
                    volumes_field(p.available_volumes, p.volumes_body),
                    new_volumes_field(p.new_volumes)))
                div.card {
                    h2.card-title { "Configuration" }
                    div.editor-row {
                        div.editor-col {
                            label { "Contents" }
                            (code_editor(p.contents_body, file_name))
                        }
                    }
                }
                div.pod-form-actions {
                    button.btn.btn-primary type="submit" { "Create" }
                }
            }
        }
    };
    shell("New pod", Some(NavItem::Services), Some(p.health), body)
}

/// One of the pod's current member containers, offered for in-place editing
/// on the Edit Pod page -- unlike [`NewContainerRow`] this isn't keyed by a
/// client-assigned `id`: `refs::pod_members` is the sole, server-computed
/// source of which members exist, so each row is addressed directly by its
/// (already-existing, unchangeable) file name. Owned `String`s rather than
/// borrows since the handler assembles each field from one of two different
/// sources (disk vs. a rejected submission's form) per row.
pub struct MemberContainerRow {
    pub file_name: String,
    pub group: String,
    pub contents_prefill: String,
    pub env_prefill: String,
    /// Whether this row's `<details>` starts open -- `false` for every row on
    /// a fresh page load (a pod's member list can get long, so each editor
    /// stays tucked away until picked), `true` only for the one member whose
    /// edit a 422 just rejected, so the error it caused is visible without
    /// having to go hunting for which row it was.
    pub open: bool,
}

/// `memberc_<file_name>_contents` -- kept in one place since the handler
/// needs the exact same key to read the field back out of the submitted form.
pub fn member_contents_field(file_name: &str) -> String {
    format!("memberc_{file_name}_contents")
}

/// `memberc_<file_name>_env` -- see [`member_contents_field`].
pub fn member_env_field(file_name: &str) -> String {
    format!("memberc_{file_name}_env")
}

/// A native `<details>` so each member's editor collapses independently with
/// no JS of its own -- unlike `.pod-picker-panel`/`.group-picker` (also
/// `<details>`, but absolutely-positioned popups), this one's revealed
/// content is a normal block in the page flow, same technique as
/// `.gitsync-disclosure`. The chevron's rotation is plain CSS keyed off the
/// `[open]` attribute (see `frontend/styles.css`), so collapsing/expanding
/// needs nothing beyond what `<details>` already does natively.
fn member_container_row(row: &MemberContainerRow) -> Markup {
    html! {
        details.newres-row open[row.open] {
            summary.newres-row-summary {
                (icon(Icon::ChevronDown))
                span.newres-row-title { (row.file_name) }
                @if !row.group.is_empty() {
                    span.pod-picker-group { (row.group) }
                }
            }
            div.editor-row {
                div.editor-col {
                    label { "Contents" }
                    (code_editor_named(
                        &row.contents_prefill,
                        EditorFileName::Fixed(&row.file_name),
                        &member_contents_field(&row.file_name),
                    ))
                }
                div.editor-col {
                    (env_var_editor_named(&row.env_prefill, &member_env_field(&row.file_name)))
                }
            }
        }
    }
}

/// The "Containers" section's members half: every container currently in
/// this pod (per `refs::pod_members`), each with its own full raw-INI +
/// env-var editor -- the same editing surface its own standalone edit page
/// offers, just embedded here so membership doesn't force a detour. Renders
/// nothing when the pod has no members yet (also why [`new_page`] never
/// calls this -- a pod being created can't have any).
fn members_field(rows: &[MemberContainerRow]) -> Markup {
    html! {
        @if !rows.is_empty() {
            div.field {
                (card_subhead("Members"))
                p.field-hint {
                    "Edit the containers already in this pod — each is saved to its own "
                    "quadlet file when you submit."
                }
                @for row in rows {
                    (member_container_row(row))
                }
            }
        }
    }
}

/// Parameters for [`edit_page`].
pub struct EditPodPage<'a> {
    pub csrf: &'a str,
    /// This pod's own URL (`/pods/<file_name>`) -- the back-link target and
    /// the form's POST target (`<base_url>/edit`).
    pub base_url: &'a str,
    pub file_name: &'a str,
    /// Every container currently in this pod, each editable in place. See
    /// [`members_field`].
    pub members: &'a [MemberContainerRow],
    /// The raw editor's body -- the file's current contents on first render,
    /// the rejected submission on a 422 redisplay.
    pub contents_body: &'a str,
    /// Newline-separated existing-container file names staged to attach
    /// (`""` on first render -- this is *new* attachments, not the pod's
    /// current members, which the picker already excludes).
    pub existing_containers_body: &'a str,
    /// Every other-pod-or-unattached container, offered by the picker --
    /// already-attached members of `file_name` are excluded, since "attach"
    /// isn't meaningful for a container already in this pod.
    pub available_containers: &'a [ContainerOption],
    pub restart_checked: bool,
    pub networks_body: &'a str,
    /// Every network not already referenced by this pod's own `[Pod]`
    /// section (see `refs::pod_own_refs`).
    pub available_networks: &'a [ResourceOption],
    pub volumes_body: &'a str,
    /// Every volume not already referenced by this pod's own `[Pod]`
    /// section.
    pub available_volumes: &'a [ResourceOption],
    /// Brand-new containers staged so far -- see [`NewPodPage::new_containers`].
    pub new_containers: &'a [NewContainerRow<'a>],
    /// Brand-new networks staged so far -- see [`NewPodPage::new_networks`].
    pub new_networks: &'a [NewNetworkRow<'a>],
    /// Brand-new volumes staged so far -- see [`NewPodPage::new_volumes`].
    pub new_volumes: &'a [NewVolumeRow<'a>],
    pub host_vars: &'a [EnvVar],
    pub error: Option<&'a str>,
    pub health: Health,
}

/// The "Edit Pod" page: the same raw INI editor every kind's edit page has,
/// plus the "New Pod" page's pickers for attaching more already-defined
/// containers/networks/volumes and defining brand-new ones, plus (via
/// [`members_field`]) a full in-place editor for every container currently
/// in the pod -- membership itself is still changed by editing each
/// container's own `Pod=` line, not this file, but that no longer requires
/// leaving the page. No file name/group fields, same as every other kind's
/// edit page (renaming/regrouping is the separate move control, not part of
/// editing).
pub fn edit_page(p: EditPodPage<'_>) -> Markup {
    let body = html! {
        (back_link(p.base_url, p.file_name))
        (page_header(&format!("Edit {}", p.file_name), html! {}))
        @if let Some(msg) = p.error { (banner(BannerKind::Error, msg)) }
        form method="post" action={(p.base_url) "/edit"} {
            (csrf_input(p.csrf))
            div.host-vars-flyout { (host_vars_panel(p.host_vars)) }
            div.pod-sections {
                (resource_card("Containers",
                    html! {
                        (members_field(p.members))
                        (containers_field(p.available_containers, p.existing_containers_body, p.restart_checked))
                    },
                    new_containers_field(p.new_containers)))
                (resource_card("Networks",
                    networks_field(p.available_networks, p.networks_body),
                    new_networks_field(p.new_networks)))
                (resource_card("Volumes",
                    volumes_field(p.available_volumes, p.volumes_body),
                    new_volumes_field(p.new_volumes)))
                div.card {
                    h2.card-title { "Configuration" }
                    div.editor-row {
                        div.editor-col {
                            label { "Contents" }
                            (code_editor(p.contents_body, EditorFileName::Fixed(p.file_name)))
                        }
                    }
                }
                div.pod-form-actions {
                    button.btn.btn-primary type="submit" { "Save" }
                }
            }
        }
    };
    shell(
        &format!("Edit {}", p.file_name),
        Some(NavItem::Services),
        Some(p.health),
        body,
    )
}

/// Wraps one resource kind's `existing`/`new` field pair in a titled
/// `.card` -- the "Containers"/"Networks"/"Volumes" section headings that
/// divide the New/Edit Pod pages into clearly bounded regions, one per
/// resource kind, echoing the same `.card`/`h2.card-title` language the unit
/// detail page's Overview card already uses. Within each card, the existing-
/// attach and brand-new subsections keep their own small [`card_subhead`]
/// ("Existing"/"New") plus [`new_containers_field`]'s divider, so the two
/// stay legible as distinct actions even nested one card together.
fn resource_card(title: &str, existing: Markup, new: Markup) -> Markup {
    html! {
        div.card {
            h2.card-title { (title) }
            (existing)
            (new)
        }
    }
}

/// A small subsection label inside a [`resource_card`] -- "Existing" above
/// the attach-picker, "New" above the brand-new rows. Styled like a `.field`
/// label (see `frontend/styles.css`'s `.card-subhead`) but not tied to one
/// input, since it introduces a whole subsection instead.
fn card_subhead(text: &str) -> Markup {
    html! { p.card-subhead { (text) } }
}

/// The "Containers" section's existing-attach half: staged chips for
/// existing containers to attach (backed by a hidden `existing_containers`
/// textarea), and an inline picker panel to build that list -- no popup,
/// part of the page's own flow. Progressively enhanced by
/// `frontend/podpicker.js`; with JS off the textarea is just a plain
/// editable box, one file name per line, which the server parses the same
/// way.
fn containers_field(
    available: &[ContainerOption],
    existing_body: &str,
    restart_checked: bool,
) -> Markup {
    html! {
        div.field data-resource-field {
            (card_subhead("Existing"))
            p.field-hint {
                "Attach containers already defined elsewhere — each is pointed at this pod \
                 with " code { "Pod=" } "."
            }

            div.staged-chips data-resource-chips {}
            textarea.input name="existing_containers" rows="3"
                placeholder="one existing container file name per line"
                data-resource-source { (existing_body) }

            details.pod-picker-panel data-resource-picker {
                summary.btn.btn-ghost.btn-sm {
                    (icon(Icon::Plus)) span { "Add existing container" }
                }
                div.pod-picker-body {
                    @if available.is_empty() {
                        p.field-hint { "No existing containers to attach yet." }
                    } @else {
                        input.input type="text" placeholder="Filter…" data-resource-filter
                            autocomplete="off" autocapitalize="off" spellcheck="false";
                        div.pod-picker-list {
                            @for c in available {
                                (picker_row(&c.file_name, &c.group, html! {
                                    @if let Some(img) = &c.image {
                                        span.pod-picker-image { (img) }
                                    }
                                    @if let Some(pod) = &c.current_pod {
                                        span.pod-picker-note { "currently in " (pod) }
                                    }
                                }))
                            }
                        }
                    }
                }
            }

            label.checkbox-line {
                input type="checkbox" name="restart_containers" checked[restart_checked];
                "Restart attached containers that are currently running, so they join this \
                 pod right away"
            }
            p.field-hint {
                "Podman only places a container into a pod when it (re)starts -- leave this \
                 off to attach without disturbing anything already running."
            }
        }
    }
}

/// The "Networks" section's existing-attach half: same staged-chips-plus-
/// inline-picker shape as [`containers_field`], but simpler rows (no image/
/// current-pod columns) and no restart checkbox -- attaching a network to a
/// pod only takes effect through the pod's own next (re)start, which is a
/// much coarser action than restarting one container and isn't offered
/// inline here.
fn networks_field(available: &[ResourceOption], body: &str) -> Markup {
    html! {
        div.field data-resource-field {
            (card_subhead("Existing"))
            p.field-hint {
                "Attach networks already defined elsewhere — each becomes a " code { "Network=" }
                " line on this pod."
            }

            div.staged-chips data-resource-chips {}
            textarea.input name="pod_networks" rows="2"
                placeholder="one existing network file name per line"
                data-resource-source { (body) }

            details.pod-picker-panel data-resource-picker {
                summary.btn.btn-ghost.btn-sm {
                    (icon(Icon::Plus)) span { "Add existing network" }
                }
                div.pod-picker-body {
                    @if available.is_empty() {
                        p.field-hint { "No existing networks to attach yet." }
                    } @else {
                        input.input type="text" placeholder="Filter…" data-resource-filter
                            autocomplete="off" autocapitalize="off" spellcheck="false";
                        div.pod-picker-list {
                            @for n in available {
                                (picker_row(&n.file_name, &n.group, html! {}))
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The "Volumes" section's existing-attach half: same shape again, but each
/// picker row also carries a destination-path input -- `Volume=` needs a
/// `SOURCE:DEST` pair, and the mount path inside the pod's containers isn't
/// something sooth can infer. Staged as `FILE=DEST` lines (checking a row
/// with no path yet auto-checks itself once you start typing one -- see
/// `frontend/podpicker.js`).
fn volumes_field(available: &[ResourceOption], body: &str) -> Markup {
    html! {
        div.field data-resource-field {
            (card_subhead("Existing"))
            p.field-hint {
                "Attach volumes already defined elsewhere, with the path to mount them at — "
                "each becomes a " code { "Volume=" } " line on this pod."
            }

            div.staged-chips data-resource-chips {}
            textarea.input name="pod_volumes" rows="2"
                placeholder="one existing_volume.volume=/mount/path per line"
                data-resource-source { (body) }

            details.pod-picker-panel data-resource-picker {
                summary.btn.btn-ghost.btn-sm {
                    (icon(Icon::Plus)) span { "Add existing volume" }
                }
                div.pod-picker-body {
                    @if available.is_empty() {
                        p.field-hint { "No existing volumes to attach yet." }
                    } @else {
                        input.input type="text" placeholder="Filter…" data-resource-filter
                            autocomplete="off" autocapitalize="off" spellcheck="false";
                        div.pod-picker-list {
                            @for v in available {
                                (picker_row(&v.file_name, &v.group, html! {
                                    input.input.input-sm type="text" placeholder="/data"
                                        data-resource-dest autocomplete="off" autocapitalize="off"
                                        spellcheck="false";
                                }))
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One inline "new container" row on the Pod pages -- a Name field, this
/// container's own raw INI editor, and its own env-var editor, each
/// independently named per row (`newc_{id}_name` / `newc_{id}_contents` /
/// `newc_{id}_env`, via `code_editor_named`/`env_var_editor_named`) so any
/// number of these can coexist on one page and submit independently. Parsed
/// back out by `handlers::pods::parse_new_containers`; `id` is a plain
/// string here (rather than the `u32` the handler works with) so the same
/// row-rendering function also produces the `__ID__`-templated row `<template>`
/// clones from client-side (see `new_containers_field`).
pub struct NewContainerRow<'a> {
    pub id: String,
    pub name_prefill: &'a str,
    pub contents_prefill: &'a str,
    pub env_prefill: &'a str,
}

fn new_container_row(
    id: &str,
    name_prefill: &str,
    contents_prefill: &str,
    env_prefill: &str,
) -> Markup {
    let name_input_id = format!("newc-{id}-name");
    let name_input_selector = format!("#{name_input_id}");
    html! {
        div.newres-row data-newres-row {
            div.newres-row-header {
                div.field {
                    label for=(name_input_id) { "Name" }
                    input.input type="text" id=(name_input_id) name={"newc_" (id) "_name"}
                        value=(name_prefill) placeholder="worker"
                        autocomplete="off" autocapitalize="off" spellcheck="false" required;
                }
                button.btn.btn-ghost.btn-sm type="button" data-newres-remove {
                    (icon(Icon::Trash)) span { "Remove this container" }
                }
            }
            div.editor-row {
                div.editor-col {
                    label { "Contents" }
                    (code_editor_named(
                        contents_prefill,
                        EditorFileName::StemSuffix { input: &name_input_selector, suffix: ".container" },
                        &format!("newc_{id}_contents"),
                    ))
                }
                div.editor-col {
                    (env_var_editor_named(env_prefill, &format!("newc_{id}_env")))
                }
            }
        }
    }
}

/// The "Containers" section's brand-new half: any number of
/// [`new_container_row`]s, each the full standalone container-create
/// experience (raw INI + env vars) embedded inline, plus an "+ Add
/// container" button. Rows are added purely client-side
/// (`frontend/podresourcerows.js`, cloning the hidden `<template>` below and
/// re-invoking `initEditors`/`initEnvVars`, both already idempotent
/// per-element) -- there's no server round trip to add one, so the only rows
/// ever server-rendered are `rows` itself (empty on first load, populated on
/// a 422 redisplay so nothing typed is lost). Its `.newres-field` top border
/// (`frontend/styles.css`) plus its own [`card_subhead`] keep it reading as
/// a distinct action from [`containers_field`]'s existing-attach picker just
/// above, even sharing one [`resource_card`] -- same separation
/// [`new_networks_field`]/[`new_volumes_field`] keep from their own
/// existing-attach fields.
fn new_containers_field(rows: &[NewContainerRow<'_>]) -> Markup {
    html! {
        div.field.newres-field {
            (card_subhead("New"))
            p.field-hint {
                "Define brand-new containers for this pod — each gets its own image, "
                code { "Pod=" } ", and (optionally) environment variables, saved as its own "
                "quadlet file."
            }
            div data-newres-list="newc" {
                @for row in rows {
                    (new_container_row(&row.id, row.name_prefill, row.contents_prefill, row.env_prefill))
                }
            }
            template data-newres-template="newc" {
                (new_container_row("__ID__", "", "[Container]\nImage=\n", ""))
            }
            button.btn.btn-ghost.btn-sm type="button" data-newres-add="newc" {
                (icon(Icon::Plus)) span { "Add container" }
            }
        }
    }
}

/// One inline "new network" row -- a Name field plus this network's own raw
/// INI editor, independently named per row (`newnet_{id}_name` /
/// `newnet_{id}_contents`). Simpler than [`new_container_row`]: a network
/// carries no env vars and doesn't reference the pod itself (the pod's own
/// `[Pod]` section gets the `Network=` line instead, same as an
/// already-existing network attach -- see `handlers::pods::apply_pod_resources`).
pub struct NewNetworkRow<'a> {
    pub id: String,
    pub name_prefill: &'a str,
    pub contents_prefill: &'a str,
}

fn new_network_row(id: &str, name_prefill: &str, contents_prefill: &str) -> Markup {
    let name_input_id = format!("newnet-{id}-name");
    let name_input_selector = format!("#{name_input_id}");
    html! {
        div.newres-row data-newres-row {
            div.newres-row-header {
                div.field {
                    label for=(name_input_id) { "Name" }
                    input.input type="text" id=(name_input_id) name={"newnet_" (id) "_name"}
                        value=(name_prefill) placeholder="frontend"
                        autocomplete="off" autocapitalize="off" spellcheck="false" required;
                }
                button.btn.btn-ghost.btn-sm type="button" data-newres-remove {
                    (icon(Icon::Trash)) span { "Remove this network" }
                }
            }
            div.editor-row {
                div.editor-col {
                    label { "Contents" }
                    (code_editor_named(
                        contents_prefill,
                        EditorFileName::StemSuffix { input: &name_input_selector, suffix: ".network" },
                        &format!("newnet_{id}_contents"),
                    ))
                }
            }
        }
    }
}

/// The "Networks" section's brand-new half -- own subhead and hint, clearly
/// apart from [`networks_field`]'s existing-attach picker above it,
/// mirroring [`new_containers_field`]'s separation from [`containers_field`].
fn new_networks_field(rows: &[NewNetworkRow<'_>]) -> Markup {
    html! {
        div.field.newres-field {
            (card_subhead("New"))
            p.field-hint {
                "Define a brand-new network for this pod — saved as its own quadlet file and "
                "attached with " code { "Network=" } "."
            }
            div data-newres-list="newnet" {
                @for row in rows {
                    (new_network_row(&row.id, row.name_prefill, row.contents_prefill))
                }
            }
            template data-newres-template="newnet" {
                (new_network_row("__ID__", "", "[Network]\n"))
            }
            button.btn.btn-ghost.btn-sm type="button" data-newres-add="newnet" {
                (icon(Icon::Plus)) span { "Add network" }
            }
        }
    }
}

/// One inline "new volume" row -- a Name field, a mount-path field (a brand
/// new volume needs a destination the same way an existing-volume picker row
/// does, see [`volumes_field`]), and this volume's own raw INI editor.
pub struct NewVolumeRow<'a> {
    pub id: String,
    pub name_prefill: &'a str,
    pub contents_prefill: &'a str,
    pub dest_prefill: &'a str,
}

fn new_volume_row(
    id: &str,
    name_prefill: &str,
    contents_prefill: &str,
    dest_prefill: &str,
) -> Markup {
    let name_input_id = format!("newvol-{id}-name");
    let name_input_selector = format!("#{name_input_id}");
    let dest_input_id = format!("newvol-{id}-dest");
    html! {
        div.newres-row data-newres-row {
            div.newres-row-header {
                div.field {
                    label for=(name_input_id) { "Name" }
                    input.input type="text" id=(name_input_id) name={"newvol_" (id) "_name"}
                        value=(name_prefill) placeholder="data"
                        autocomplete="off" autocapitalize="off" spellcheck="false" required;
                }
                div.field {
                    label for=(dest_input_id) { "Mount path" }
                    input.input type="text" id=(dest_input_id) name={"newvol_" (id) "_dest"}
                        value=(dest_prefill) placeholder="/data"
                        autocomplete="off" autocapitalize="off" spellcheck="false" required;
                }
                button.btn.btn-ghost.btn-sm type="button" data-newres-remove {
                    (icon(Icon::Trash)) span { "Remove this volume" }
                }
            }
            div.editor-row {
                div.editor-col {
                    label { "Contents" }
                    (code_editor_named(
                        contents_prefill,
                        EditorFileName::StemSuffix { input: &name_input_selector, suffix: ".volume" },
                        &format!("newvol_{id}_contents"),
                    ))
                }
            }
        }
    }
}

/// The "Volumes" section's brand-new half -- own subhead and hint, clearly
/// apart from [`volumes_field`]'s existing-attach picker above it, mirroring
/// [`new_containers_field`]'s separation from [`containers_field`].
fn new_volumes_field(rows: &[NewVolumeRow<'_>]) -> Markup {
    html! {
        div.field.newres-field {
            (card_subhead("New"))
            p.field-hint {
                "Define a brand-new volume for this pod, with the path to mount it at — saved "
                "as its own quadlet file and attached with " code { "Volume=" } "."
            }
            div data-newres-list="newvol" {
                @for row in rows {
                    (new_volume_row(&row.id, row.name_prefill, row.contents_prefill, row.dest_prefill))
                }
            }
            template data-newres-template="newvol" {
                (new_volume_row("__ID__", "", "[Volume]\n", ""))
            }
            button.btn.btn-ghost.btn-sm type="button" data-newres-add="newvol" {
                (icon(Icon::Plus)) span { "Add volume" }
            }
        }
    }
}

/// One picker row shared by the containers/networks/volumes pickers --
/// checkbox, file name, group (if filed under one), and whatever extra
/// markup that picker's rows carry (image/current-pod for containers, a
/// destination-path input for volumes, nothing for networks).
fn picker_row(file_name: &str, group: &str, extra: Markup) -> Markup {
    html! {
        label.pod-picker-row data-resource-row={(file_name) " " (group)} {
            input type="checkbox" value=(file_name);
            span.pod-picker-name { (file_name) }
            @if !group.is_empty() {
                span.pod-picker-group { (group) }
            }
            (extra)
        }
    }
}
