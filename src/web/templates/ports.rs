use maud::{Markup, html};

use super::{NavItem, csrf_input, page_header, plain_status_badge, shell, status_badge};
use crate::health::Health;
use crate::quadlet::naming;
use crate::quadlet::ports::PortMapping;
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
    pub containers: Vec<UnitRef<'a>>,
    /// True when `containers` is narrowed to the members that declare the
    /// container port (`ExposePort=` or image `EXPOSE`); false when none
    /// does and `containers` is every member.
    pub matched: bool,
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
                        Some(targets) => {
                            @if targets.containers.is_empty() {
                                span.muted title="No container has Pod= pointing at this pod" {
                                    "no containers in pod"
                                }
                            }
                            @for m in &targets.containers {
                                div {
                                    a href=(m.href) { (m.file_name) }
                                    " "
                                    (plain_status_badge(m.status))
                                    @if let Some(image) = &m.unpulled {
                                        " "
                                        form.inline-form hx-post="/ports/pull" hx-target="#ports-rows"
                                            hx-swap="innerHTML" hx-disabled-elt="find button" {
                                            (csrf_input(csrf))
                                            input type="hidden" name="file_name" value=(m.file_name);
                                            button.btn.btn-sm type="submit"
                                                title={"Image " (image) " isn't pulled yet, so the ports it exposes are unknown. Pull it now."} {
                                                "Pull image"
                                            }
                                            span.htmx-indicator.muted { " pulling…" }
                                        }
                                    }
                                    @if let Some((_, msg)) = pull_error.filter(|(f, _)| *f == m.file_name) {
                                        div.cell-secondary { "pull failed: " (msg) }
                                    }
                                }
                            }
                            div.cell-secondary {
                                "via pod " a href=(row.owner.href) { (row.owner.file_name) }
                            }
                            @if !targets.matched && targets.containers.len() > 1 {
                                div.cell-secondary title="Add ExposePort= to the container that serves it, or pull its image so its EXPOSE can be read" {
                                    "none declares port " (row.mapping.container_port) "/" (row.mapping.protocol.as_str())
                                }
                            }
                        }
                    }
                }
                td { (status_badge(&naming::service_name(&row.owner.file_name), row.owner.status)) }
            }
        }
    }
}

pub fn ports_page(rows: &[PortRow], csrf: &str, health: Health) -> Markup {
    let body = html! {
        (page_header("Ports", html! {}))
        p.page-meta {
            "Declared " code { "PublishPort=" } " entries across Containers and Pods. "
            "A pod's ports are shown against the member that declares the container port ("
            code { "ExposePort=" } " or its image's " code { "EXPOSE" } "), else every member."
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
