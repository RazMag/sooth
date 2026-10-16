use maud::{Markup, html};

use super::icons::{Icon, icon};
use super::{
    NavItem, container_port_label, csrf_input, info_tip, page_header, plain_status_badge, port_why,
    shell, status_badge,
};
use crate::health::Health;
use crate::quadlet::naming;
use crate::quadlet::ports::{ExposedPort, PortMapping, Why};
use crate::systemd::UnitStatus;

/// A unit as the Ports table shows it: its file name, detail-page URL, and
/// live status.
pub struct UnitRef<'a> {
    pub file_name: String,
    pub href: String,
    pub status: &'a UnitStatus,
    /// Set on a pod member whose image isn't in local storage (so its
    /// `EXPOSE` can't be read): the image a "Pull image" button would fetch.
    pub unpulled: Option<String>,
}

/// The containers behind a pod-published port.
pub struct PodTargets<'a> {
    /// The member(s) the port reaches, each with why (`ports::serving`):
    /// the one it's assigned to by hand, else those declaring its container
    /// port. Empty = none does, so it stays on the pod.
    pub serving: Vec<(UnitRef<'a>, Why)>,
    /// Members whose image isn't pulled, so their `EXPOSE` is unknown --
    /// offered a "Pull image" while the port stays on the pod.
    pub unpulled: Vec<UnitRef<'a>>,
    /// Every member's file name -- the choices of the assign picker.
    pub members: Vec<String>,
}

/// One rendered row, fully resolved by the handler (owner, the containers
/// behind a pod's port, conflicting units) so the template does no lookups
/// of its own.
pub struct PortRow<'a> {
    pub mapping: &'a PortMapping,
    /// The quadlet declaring the `PublishPort=`.
    pub owner: UnitRef<'a>,
    /// `Some` when the owner is a pod: the member container(s) the port
    /// reaches (see [`PodTargets`]).
    pub members: Option<PodTargets<'a>>,
    /// Other units whose host port overlaps this one, with a link when the
    /// unit is known. Empty = no conflict.
    pub conflicts: Vec<(String, Option<String>)>,
}

/// Just the `<tbody>` rows -- the payload of `GET /ports/rows`. A unit
/// starting or stopping flips its Host Port cell between a greyed pill and a
/// live link, but that's driven by `is_active()` at render time and the
/// per-row status badge's SSE swap doesn't touch it. The table has no menus
/// or open state, so the whole `<tbody>` re-fetches this on `sse:any-status`
/// (and `sse:units-changed` for added/removed mappings) -- which is also what
/// keeps pod members' (non-SSE) badges current.
///
/// `pull_error` is `(member file name, podman's message)` after a failed
/// `POST /ports/pull`, shown under that member.
pub fn ports_rows(rows: &[PortRow], csrf: &str, pull_error: Option<(&str, &str)>) -> Markup {
    html! {
        @for row in rows {
            tr class=[(!row.conflicts.is_empty()).then_some("row-collision")] {
                td title=(row.mapping.raw) {
                    @match row.mapping.host_port {
                        Some(r) if r.start == r.end => {
                            @if row.owner.status.is_active() {
                                span data-host-port=(r.start.to_string()) { (r.start) }
                            } @else {
                                span.port-static title="Service not running" { (r.start) }
                            }
                        }
                        Some(r) => { (format!("{}-{}", r.start, r.end)) }
                        None => { span.muted { "dynamic" } }
                    }
                    @if !row.conflicts.is_empty() {
                        div.cell-secondary {
                            "conflicts with "
                            @for (i, (name, href)) in row.conflicts.iter().enumerate() {
                                @if i > 0 { ", " }
                                @match href {
                                    Some(href) => { a href=(href) { (name) } }
                                    None => { (name) }
                                }
                            }
                        }
                    }
                }
                td { (row.mapping.protocol.as_str()) }
                td { (row.mapping.container_port) }
                td {
                    @match &row.members {
                        None => { a href=(row.owner.href) { (row.owner.file_name) } }
                        Some(targets) => { (pod_targets(row, targets, csrf, pull_error)) }
                    }
                }
                td { (status_badge(&naming::service_name(&row.owner.file_name), row.owner.status)) }
            }
        }
    }
}

/// The Container cell of a pod-published port, in one of three shapes:
/// the member(s) it was matched to automatically, the member it's assigned
/// to by hand, or -- when no member declares it -- the pod itself. Each
/// target's info icon says why it's there ([`port_why`]); the second line
/// names the pod and carries the [`assign_picker`].
fn pod_targets(
    row: &PortRow,
    targets: &PodTargets,
    csrf: &str,
    pull_error: Option<(&str, &str)>,
) -> Markup {
    let pod = &row.owner;
    let port = container_port_label(row.mapping);
    let pod_chip = html! {
        a.chip.chip-pod href=(pod.href) {
            (icon(Icon::Pod)) (naming::stem(&pod.file_name))
        }
    };
    if targets.members.is_empty() {
        return html! {
            (pod_chip)
            div.cell-secondary title="No container has Pod= pointing at this pod" {
                "no containers in pod"
            }
        };
    }
    html! {
        @if targets.serving.is_empty() {
            div.port-target {
                (pod_chip)
                " "
                (info_tip(&port_why(None, "", &pod.file_name, &port)))
            }
            @for m in &targets.unpulled {
                @if let Some(image) = &m.unpulled {
                    div.cell-secondary {
                        form.inline-form hx-post="/ports/pull" hx-target="#ports-rows"
                            hx-swap="innerHTML" hx-disabled-elt="find button" {
                            (csrf_input(csrf))
                            input type="hidden" name="file_name" value=(m.file_name);
                            (m.file_name) "'s image isn't pulled "
                            button.btn.btn-sm type="submit"
                                title={"Image " (image) " isn't pulled yet, so the ports it exposes are unknown. Pull it now."} {
                                "Pull image"
                            }
                            span.htmx-indicator.muted { " pulling…" }
                        }
                        @if let Some((_, msg)) = pull_error.filter(|(f, _)| *f == m.file_name) {
                            div { "pull failed: " (msg) }
                        }
                    }
                }
            }
            div.cell-secondary.port-route {
                "no member declares " (port)
                (assign_picker(row, targets, csrf))
            }
        } @else {
            @for (m, why) in &targets.serving {
                div.port-target {
                    a href=(m.href) { (m.file_name) }
                    " "
                    (plain_status_badge(m.status))
                    " "
                    (info_tip(&port_why(Some(why), &m.file_name, &pod.file_name, &port)))
                }
            }
            div.cell-secondary.port-route {
                "via " (pod_chip)
                (assign_picker(row, targets, csrf))
            }
        }
    }
}

/// Which container a pod port reaches, as a quiet inline control posted on
/// change to `/ports/assign`, which records the choice in that member's
/// quadlet as sooth-only bookkeeping (`ports::ASSIGN_KEY` -- podman ignores
/// it). A native `<select>` laid transparently over a label that reads
/// differently per state: "assigned" (accent) once a member is picked by
/// hand, "auto" while matched automatically, and a dashed "assign…" while
/// the port sits on the pod with no member declaring it.
fn assign_picker(row: &PortRow, targets: &PodTargets, csrf: &str) -> Markup {
    let Some(port) = ExposedPort::of_mapping(row.mapping) else {
        return html! {};
    };
    let assigned = targets
        .serving
        .first()
        .filter(|(_, why)| *why == Why::Assigned)
        .map(|(m, _)| m.file_name.as_str());
    let (state, label) = match (assigned, targets.serving.is_empty()) {
        (Some(_), _) => ("is-assigned", "assigned"),
        (None, false) => ("is-auto", "auto"),
        (None, true) => ("is-unassigned", "assign…"),
    };
    html! {
        form.inline-form.port-assign hx-post="/ports/assign" hx-trigger="change"
            hx-target="#ports-rows" hx-swap="innerHTML" {
            (csrf_input(csrf))
            input type="hidden" name="pod" value=(row.owner.file_name);
            input type="hidden" name="port" value=(port.spec());
            span class={"port-assign-control " (state)}
                title="Pick the container this pod port reaches. Saved as X-Sooth-ServesPort= in that container's quadlet; only sooth reads it, podman runs the pod unchanged." {
                span.port-assign-label { (label) }
                (icon(Icon::ChevronDown))
                select name="member" aria-label={"Container serving " (container_port_label(row.mapping))} {
                    option value="" selected[assigned.is_none()] { "auto (match ExposeHostPort= / EXPOSE)" }
                    @for m in &targets.members {
                        option value=(m) selected[assigned == Some(m.as_str())] { (m) }
                    }
                }
            }
        }
    }
}

pub fn ports_page(rows: &[PortRow], csrf: &str, health: Health) -> Markup {
    let body = html! {
        (page_header("Ports", html! {}))
        p.page-meta {
            "Declared " code { "PublishPort=" } " entries across Containers and Pods. "
            "A pod's ports are shown against the member you assign them to, else the one that declares the container port ("
            code { "ExposeHostPort=" } " or its image's " code { "EXPOSE" } "), else the pod itself. Hover " span.info-tip { (icon(Icon::Info)) } " to see why."
        }
        @if rows.is_empty() {
            p.empty { "No published ports declared yet." }
        } @else {
            div.table-wrap {
                table.data-table {
                    thead {
                        tr {
                            th { "Host Port" }
                            th { "Protocol" }
                            th { "Container Port" }
                            th { "Container" }
                            th { "Status" }
                        }
                    }
                    tbody id="ports-rows" hx-get="/ports/rows"
                        hx-trigger="sse:any-status delay:300ms, sse:units-changed delay:300ms"
                        hx-swap="innerHTML" {
                        (ports_rows(rows, csrf, None))
                    }
                }
            }
        }
    };
    shell("Ports", Some(NavItem::Ports), Some(health), body)
}
