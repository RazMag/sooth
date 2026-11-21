use maud::{Markup, html};

use super::icons::{Icon, icon};
use super::{
    BannerKind, NavItem, banner, container_port_label, csrf_input, info_tip, page_header,
    plain_status_badge, port_why, shell, status_badge,
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

/// A `Network=host` container and the ports recorded for it
/// (`ports::host_ports`) -- one row of the Ports screen's "Host network"
/// table, where they're added and removed.
pub struct HostNet<'a> {
    pub unit: UnitRef<'a>,
    pub ports: Vec<ExposedPort>,
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
                td {
                    @if row.mapping.host_network {
                        span.muted title="Network=host: the container listens on the host port itself" { "host network" }
                    } @else {
                        (row.mapping.container_port)
                    }
                }
                td {
                    @match &row.members {
                        None => {
                            a href=(row.owner.href) { (row.owner.file_name) }
                            @if row.mapping.host_network {
                                " "
                                (info_tip(&host_port_why(&row.owner.file_name, &row.mapping.container_port)))
                            }
                        }
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
/// change to `/ports/assign`, which records the choice in sooth's own
/// `portnotes` store, not the quadlet (podman never sees it). A native `<select>` laid transparently over a label that reads
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
                title="Pick the container this pod port reaches. Saved by sooth, not in the quadlet; podman runs the pod unchanged." {
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

/// Why a host-network port is listed -- the tooltip on its Ports screen
/// row and its Services list pill.
pub fn host_port_why(file_name: &str, port: &str) -> String {
    format!(
        "{file_name} runs with Network=host, so it listens on host port {port} directly. \
         Recorded by hand on the Ports screen (saved by sooth, not in the quadlet)."
    )
}

/// One port claimed by more than one unit, for the conflict card.
struct Clash<'r, 'a> {
    label: String,
    rows: Vec<&'r PortRow<'a>>,
}

/// Groups the conflicting rows: rows that name each other land in one
/// clash, keyed by the set of units and the protocol.
fn clashes<'r, 'a>(rows: &'r [PortRow<'a>]) -> Vec<Clash<'r, 'a>> {
    let mut out: Vec<(Vec<String>, Clash)> = Vec::new();
    for row in rows.iter().filter(|r| !r.conflicts.is_empty()) {
        let mut key: Vec<String> = row.conflicts.iter().map(|(n, _)| n.clone()).collect();
        key.push(row.owner.file_name.clone());
        key.push(row.mapping.protocol.as_str().to_string());
        key.sort();
        key.dedup();
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, clash)) => clash.rows.push(row),
            None => {
                let port = match row.mapping.host_port {
                    Some(r) if r.start == r.end => r.start.to_string(),
                    Some(r) => format!("{}-{}", r.start, r.end),
                    None => "?".to_string(),
                };
                let label = format!("{port}/{}", row.mapping.protocol.as_str());
                out.push((
                    key,
                    Clash {
                        label,
                        rows: vec![row],
                    },
                ));
            }
        }
    }
    out.into_iter().map(|(_, c)| c).collect()
}

fn names(rows: &[&PortRow]) -> String {
    let names: Vec<&str> = rows.iter().map(|r| r.owner.file_name.as_str()).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What's going on with one clash, in a sentence.
fn clash_story(rows: &[&PortRow]) -> String {
    let (up, down): (Vec<&PortRow>, Vec<&PortRow>) =
        rows.iter().partition(|r| r.owner.status.is_active());
    match (up.as_slice(), down.len()) {
        ([holder], 0) => format!("{} holds it.", holder.owner.file_name),
        ([holder], n) => format!(
            "{} holds it. {} {} stopped and would fail to bind when started.",
            holder.owner.file_name,
            names(&down),
            if n == 1 { "is" } else { "are" }
        ),
        ([], _) => "None of them is running; only the first to start will bind it.".to_string(),
        _ => format!("{} all publish it, but only one can bind it.", names(&up)),
    }
}

/// The card above the Ports table: each host port claimed more than once,
/// who holds it, and a way to edit the others. Re-fetched with the rows
/// (`GET /ports/conflicts`); empty when there's no clash.
pub fn conflicts_card(rows: &[PortRow]) -> Markup {
    let clashes = clashes(rows);
    html! {
        @for clash in &clashes {
            section.card.clash-card {
                h2.card-title {
                    code { (clash.label) } " is claimed "
                    (match clash.rows.len() { 2 => "twice".to_string(), n => format!("{n} times") })
                }
                p.clash-story { (clash_story(&clash.rows)) }
                div.clash-claims {
                    @for row in &clash.rows {
                        span.clash-claim {
                            a href=(row.owner.href) { (row.owner.file_name) }
                            " · " code { (row.mapping.raw) }
                            @if !row.owner.status.is_active() {
                                " " a.btn.btn-sm href={(row.owner.href) "/edit"} { "Change port…" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One published port on another machine: what the table needs, already
/// looked up.
pub struct RemotePortRow {
    pub mapping: PortMapping,
    pub owner: String,
    pub href: String,
    pub status: UnitStatus,
    /// Other units there whose host port overlaps this one.
    pub conflicts: Vec<String>,
}

/// Another machine's share of the Ports page.
pub struct RemotePorts {
    pub id: String,
    pub name: String,
    /// Where a browser reaches what it publishes, for the port links.
    pub host: Option<String>,
    pub answering: bool,
    /// Why the rows are old or missing, when it didn't answer.
    pub note: Option<String>,
    /// `None` when it has never listed its units.
    pub rows: Option<Vec<RemotePortRow>>,
}

/// The heading over one machine's ports, on a page that shows several.
fn machine_title(name: &str, answering: bool, this_hub: bool) -> Markup {
    html! {
        h2.ports-machine-title {
            span.machine-dot.is-online[answering].is-offline[!answering] aria-hidden="true" {}
            (name)
            @if this_hub { span.chip.chip-muted { "this hub" } }
        }
    }
}

/// Every other machine's ports -- also `GET /ports/machines`, re-fetched
/// while any is in scope since they don't push their changes yet.
pub fn remote_ports(machines: &[RemotePorts]) -> Markup {
    html! {
        @for m in machines {
            section.ports-machine data-machine=(m.id) {
                (machine_title(&m.name, m.answering, false))
                @if let Some(note) = &m.note { (banner(BannerKind::Warn, note)) }
                @match &m.rows {
                    None => {},
                    Some(rows) if rows.is_empty() => p.empty { "No published ports declared." },
                    Some(rows) => div.table-wrap {
                        table.data-table {
                            thead { tr {
                                th { "Host Port" } th { "Protocol" } th { "Container Port" }
                                th { "Container" } th { "Status" }
                            } }
                            tbody { @for row in rows { (remote_port_row(row, m.host.as_deref())) } }
                        }
                    },
                }
            }
        }
    }
}

fn remote_port_row(row: &RemotePortRow, host: Option<&str>) -> Markup {
    html! {
        tr class=[(!row.conflicts.is_empty()).then_some("row-collision")] {
            td title=(row.mapping.raw) {
                @match row.mapping.host_port {
                    Some(r) if r.start == r.end => {
                        @if row.status.is_active() && host.is_some() {
                            span data-host-port=(r.start.to_string()) data-host=[host] { (r.start) }
                        } @else {
                            span.port-static title="Service not running" { (r.start) }
                        }
                    }
                    Some(r) => { (format!("{}-{}", r.start, r.end)) }
                    None => { span.muted { "dynamic" } }
                }
                @if !row.conflicts.is_empty() {
                    div.cell-secondary { "conflicts with " (row.conflicts.join(", ")) }
                }
            }
            td { (row.mapping.protocol.as_str()) }
            td {
                @if row.mapping.host_network {
                    span.muted { "host network" }
                } @else {
                    (row.mapping.container_port)
                }
            }
            td { a href=(row.href) { (row.owner) } }
            td { (plain_status_badge(&row.status)) }
        }
    }
}

pub struct PortsPage<'a> {
    /// This hub's own ports, when its machine is in scope.
    pub local: Option<(&'a [PortRow<'a>], &'a [HostNet<'a>])>,
    /// The hub's name, to head its ports when other machines are shown too.
    pub hub_name: &'a str,
    pub remotes: &'a [RemotePorts],
    /// `/ports/machines` in the page's scope.
    pub remotes_route: &'a str,
    pub scopes: Option<Vec<super::scope::ScopeChoice>>,
    pub csrf: &'a str,
    pub health: Health,
}

pub fn ports_page(p: &PortsPage) -> Markup {
    let body = html! {
        (page_header("Ports", html! {}))
        @if let Some(choices) = &p.scopes { (super::scope::scope_switch(choices)) }
        @if let Some((rows, hosts)) = p.local {
            div #ports-conflicts hx-get="/ports/conflicts"
                hx-trigger="sse:any-status delay:300ms, sse:units-changed delay:300ms"
                hx-swap="innerHTML" {
                (conflicts_card(rows))
            }
            @if !p.remotes.is_empty() { (machine_title(p.hub_name, true, true)) }
            (ports_content(rows, hosts, p.csrf, None))
        }
        @if !p.remotes.is_empty() {
            div #ports-machines hx-get=(p.remotes_route)
                hx-trigger="sse:machines-changed, every 30s" hx-swap="innerHTML" {
                (remote_ports(p.remotes))
            }
        }
    };
    shell("Ports", Some(NavItem::Ports), Some(p.health), body)
}

/// Everything below the Ports screen's header -- also the response to
/// `POST /ports/host`, which swaps it whole so a first host port brings the
/// table in with it, and a rejected one shows `error` above the form.
pub fn ports_content(
    rows: &[PortRow],
    hosts: &[HostNet],
    csrf: &str,
    error: Option<&str>,
) -> Markup {
    html! {
        div #ports-content {
            p.page-meta {
                "Declared " code { "PublishPort=" } " entries across Containers and Pods, plus the ports recorded for "
                code { "Network=host" } " containers. "
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
            @if !hosts.is_empty() {
                (host_network(hosts, csrf, error))
            }
        }
    }
}

/// The "Host network" section: each `Network=host` container with the ports
/// recorded for it, each removable, and a field to add one. A host-network
/// container has no `PublishPort=` saying which host ports it takes, so
/// these are the only way its ports reach the table above, the Services
/// list and its detail page.
fn host_network(hosts: &[HostNet], csrf: &str, error: Option<&str>) -> Markup {
    let form_attrs = |m: Markup| {
        html! {
            form.inline-form hx-post="/ports/host" hx-target="#ports-content"
                hx-swap="outerHTML" hx-disabled-elt="find button" {
                (csrf_input(csrf))
                (m)
            }
        }
    };
    html! {
        h2 #host-network { "Host network" }
        p.page-meta {
            "These containers run with " code { "Network=host" } " and bind the host's ports directly, so there is no "
            code { "PublishPort=" } " to read. Add the ports each one listens on to list them above, on the Services list "
            "and on its page, and to check them for conflicts. Sooth saves them itself, not in the "
            "quadlet, so podman never sees them and nothing is restarted."
        }
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }
        div.table-wrap {
            table.data-table {
                thead {
                    tr {
                        th { "Container" }
                        th { "Status" }
                        th { "Host Ports" }
                        th { "Add port" }
                    }
                }
                tbody {
                    @for h in hosts {
                        tr {
                            td { a href=(h.unit.href) { (h.unit.file_name) } }
                            td { (plain_status_badge(h.unit.status)) }
                            td {
                                @if h.ports.is_empty() {
                                    span.muted { "none recorded" }
                                } @else {
                                    span.port-list {
                                        @for p in &h.ports {
                                            span.host-port {
                                                span.mono { (p.spec()) }
                                                (form_attrs(html! {
                                                    input type="hidden" name="file_name" value=(h.unit.file_name);
                                                    input type="hidden" name="port" value=(p.spec());
                                                    input type="hidden" name="remove" value="1";
                                                    button.host-port-remove type="submit"
                                                        title={"Remove " (p.spec())}
                                                        aria-label={"Remove port " (p.spec()) " from " (h.unit.file_name)} {
                                                        (icon(Icon::X))
                                                    }
                                                }))
                                            }
                                        }
                                    }
                                }
                            }
                            td {
                                (form_attrs(html! {
                                    input type="hidden" name="file_name" value=(h.unit.file_name);
                                    span.host-port-add {
                                        input.input.input-sm type="text" name="port" required
                                            placeholder="8080 or 53/udp" autocomplete="off" spellcheck="false"
                                            pattern="\\s*[0-9]{1,5}(-[0-9]{1,5})?(/(tcp|udp))?\\s*"
                                            title="A port (8080), a range (8000-8010), optionally /tcp or /udp"
                                            aria-label={"Port " (h.unit.file_name) " listens on"};
                                        button.btn.btn-sm type="submit" { (icon(Icon::Plus)) "Add" }
                                    }
                                }))
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quadlet::ports::{HostPortRange, Protocol};

    fn mapping(file: &str, raw: &str) -> PortMapping {
        PortMapping {
            file_name: file.into(),
            host_port: Some(HostPortRange {
                start: 8080,
                end: 8080,
            }),
            protocol: Protocol::Tcp,
            container_port: "8080".into(),
            raw: raw.into(),
            host_network: false,
        }
    }

    #[test]
    fn conflict_card_names_the_holder_and_the_one_that_would_fail() {
        let mut up = UnitStatus::not_found();
        up.active_state = "active".into();
        let down = UnitStatus::not_found();
        let (a, b) = (
            mapping("qbit.container", "8080:8080"),
            mapping("paperless.container", "8080:8000"),
        );
        let unit = |f: &str, st| UnitRef {
            file_name: f.into(),
            href: format!("/containers/{f}"),
            status: st,
            unpulled: None,
        };
        let rows = vec![
            PortRow {
                mapping: &a,
                owner: unit("qbit.container", &up),
                members: None,
                conflicts: vec![("paperless.container".into(), None)],
            },
            PortRow {
                mapping: &b,
                owner: unit("paperless.container", &down),
                members: None,
                conflicts: vec![("qbit.container".into(), None)],
            },
        ];
        let html = conflicts_card(&rows).into_string();
        assert_eq!(html.matches("clash-card").count(), 1, "{html}");
        assert!(html.contains("8080/tcp</code> is claimed twice"));
        assert!(html.contains("qbit.container holds it. paperless.container is stopped and would fail to bind when started."));
        assert!(html.contains(r#"href="/containers/paperless.container/edit""#));
        assert!(conflicts_card(&rows[..0]).into_string().is_empty());
    }

    #[test]
    fn another_machines_ports_link_to_its_host_and_its_unit_pages() {
        let mut up = UnitStatus::not_found();
        up.active_state = "active".into();
        let row = |f: &str, status: &UnitStatus, conflicts: Vec<String>| RemotePortRow {
            mapping: mapping(f, "8080:8080"),
            owner: f.into(),
            href: format!("/machines/pi/units/{f}"),
            status: status.clone(),
            conflicts,
        };
        let html = remote_ports(&[RemotePorts {
            id: "pi".into(),
            name: "Pi".into(),
            host: Some("10.0.0.5".into()),
            answering: true,
            note: None,
            rows: Some(vec![
                row("web.container", &up, vec!["old.container".into()]),
                row(
                    "old.container",
                    &UnitStatus::not_found(),
                    vec!["web.container".into()],
                ),
            ]),
        }])
        .into_string();
        assert!(html.contains(r#"data-host-port="8080" data-host="10.0.0.5""#));
        assert!(html.contains(r#"href="/machines/pi/units/web.container""#));
        assert!(html.contains("conflicts with old.container"));
        assert!(
            !html.contains("sse-swap"),
            "no live badges for another machine"
        );
        assert_eq!(
            html.matches("data-host-port").count(),
            1,
            "only a running service links"
        );

        let silent = remote_ports(&[RemotePorts {
            id: "pi".into(),
            name: "Pi".into(),
            host: None,
            answering: false,
            note: Some("Pi isn't answering (refused).".into()),
            rows: None,
        }])
        .into_string();
        assert!(silent.contains("Pi isn't answering (refused)."));
        assert!(!silent.contains("<table"));
    }
}
