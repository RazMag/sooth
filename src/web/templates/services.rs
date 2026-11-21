//! The Services home page: the actual running workloads (Containers + Pods)
//! combined into one list, with a stats bar on top. Volumes/Networks/Images/
//! Ports are resource sections, not "services" -- they stay their own sidebar
//! entries, one click away, rather than cluttering this page.

use maud::{Markup, html};

use super::list::{Column, ListContext, ListSpec, RowCtx, TableBody};
use super::{
    Icon, NavItem, icon, ports_cell_live, ports_summary, shell, split_ports_cell_live,
    split_ports_summary,
};
use crate::health::Health;
use crate::quadlet::UnitKind;

/// A pod's published ports show on the member(s) serving them, the rest on
/// the pod itself (see `ListContext::ports`); any other row shows its own.
fn ports_cell(ctx: &RowCtx) -> Markup {
    let active = ctx.status.is_active();
    match (ctx.split_ports, ctx.live) {
        (Some(split), true) => split_ports_cell_live(ctx.unit, ctx.status, split),
        (None, true) => ports_cell_live(ctx.unit, ctx.status),
        (Some(split), false) => split_ports_summary(ctx.unit, active, split),
        (None, false) => ports_summary(ctx.unit, active),
    }
}

/// A container's `Image=` as written (an `.image`/`.build` quadlet name or a
/// registry reference); blank for a pod.
fn image_cell(ctx: &RowCtx) -> Markup {
    html! {
        @if let Some(image) = ctx.unit.section("Container").and_then(|s| s.get("Image")) {
            code.cell-code { (super::soft_wrap_path(image)) }
        }
    }
}

pub const COLUMNS: &[Column] = &[
    Column {
        header: "Image",
        cell: image_cell,
    },
    Column {
        header: "Ports",
        cell: ports_cell,
    },
];

pub const SPEC: ListSpec = ListSpec {
    title: "Services",
    active_nav: Some(NavItem::Services),
    kinds: &[UnitKind::Container, UnitKind::Pod],
    columns: COLUMNS,
    new_href: "/containers/new",
    empty_hint: "No services yet — create a container or pod to get started.",
    state_filter: true,
    service_columns: true,
};

pub struct Stats {
    pub total: usize,
    pub running: usize,
    pub failed: usize,
}

fn stats_bar(stats: &Stats) -> Markup {
    html! {
        div.stat-grid {
            div.stat-card {
                div.stat-card-label { "Total" }
                div.stat-card-value { (stats.total) }
            }
            div.stat-card {
                div.stat-card-label { "Running" }
                div.stat-card-value { (stats.running) }
            }
            div class=(if stats.failed > 0 { "stat-card stat-card-alert" } else { "stat-card" }) {
                div.stat-card-label { "Failed" }
                div.stat-card-value { (stats.failed) }
            }
        }
    }
}

pub fn counts_fragment(stats: &Stats) -> Markup {
    stats_bar(stats)
}

/// The bulk bar, stuck to the bottom of the window under the table: hidden until a row's checkbox is
/// ticked (`frontend/bulk.js` fills `units` and the count).
fn bulk_bar(csrf: &str) -> Markup {
    html! {
        form #bulk-bar .bulk-bar hidden hx-post="/services/bulk" hx-target="#bulk-result" hx-swap="innerHTML" {
            (super::csrf_input(csrf))
            input type="hidden" name="units" value="";
            span.bulk-count { span data-bulk-count { "0" } " selected" }
            button.btn.btn-sm type="submit" name="action" value="restart" { (icon(Icon::Restart)) span { "Restart" } }
            button.btn.btn-sm type="submit" name="action" value="start" { (icon(Icon::Play)) span { "Start" } }
            button.btn.btn-sm type="submit" name="action" value="stop" { (icon(Icon::Stop)) span { "Stop" } }
            button.btn.btn-sm.btn-ghost type="button" data-bulk-clear { "Clear" }
            span #bulk-result .bulk-result role="status" {}
        }
    }
}

/// The bar's one-line answer: "Restarted 2 units." plus any that failed.
pub fn bulk_result(ok: usize, failed: &[(String, String)], done: &str) -> Markup {
    html! {
        @if ok > 0 {
            span { (done) " " (ok) (if ok == 1 { " unit." } else { " units." }) }
        }
        @for (name, err) in failed {
            span.bulk-error { " " @if !name.is_empty() { strong { (name) } ": " } (err) }
        }
    }
}

/// How a machine's header row reads.
pub enum HeaderState<'a> {
    Live {
        count: usize,
    },
    /// It didn't answer; its rows are what it last listed.
    Stale {
        count: usize,
        ago: String,
        reason: &'a str,
    },
    /// It didn't answer, and hasn't since the hub started.
    Unreachable {
        reason: &'a str,
    },
}

pub struct MachineHeader<'a> {
    pub id: &'a str,
    pub name: &'a str,
    /// This hub's own machine.
    pub local: bool,
    pub state: HeaderState<'a>,
}

/// The row heading one machine's units on a page that lists several. Its
/// Retry redraws the whole table (`retry` is the rows route).
pub fn machine_row(h: &MachineHeader, colspan: usize, retry: &str) -> Markup {
    let online = !matches!(
        h.state,
        HeaderState::Stale { .. } | HeaderState::Unreachable { .. }
    );
    let units = |n: usize| {
        if n == 1 {
            "1 service".to_string()
        } else {
            format!("{n} services")
        }
    };
    html! {
        tr.machine-row data-machine=(h.id) {
            td colspan=(colspan) {
                div.machine-head {
                    span.machine-dot.is-online[online].is-offline[!online] aria-hidden="true" {}
                    a.machine-head-name href={"/services?m=" (h.id)} { (h.name) }
                    @if h.local { span.chip.chip-muted { "this hub" } }
                    @match &h.state {
                        HeaderState::Live { count } => span.muted { (units(*count)) },
                        HeaderState::Stale { count, ago, reason } => {
                            span.muted { (units(*count)) }
                            span.machine-head-warn {
                                "Offline: " (reason) ". Showing what it reported " (ago) "."
                            }
                        }
                        HeaderState::Unreachable { reason } => {
                            span.machine-head-warn { "Offline: " (reason) "." }
                        }
                    }
                    @if !online {
                        button.btn.btn-sm.btn-ghost type="button" hx-get=(retry)
                            hx-target={"#" (super::list::ROWS_ID)} hx-swap="innerHTML" {
                            (icon(Icon::Refresh)) span { "Retry" }
                        }
                    }
                }
            }
        }
        @if let HeaderState::Unreachable { .. } = h.state {
            tr.machine-empty data-machine=(h.id) {
                td colspan=(colspan) .empty {
                    "Nothing to show yet: it hasn't answered since the hub started."
                }
            }
        }
    }
}

pub struct ServicesPage<'a> {
    pub stats: &'a Stats,
    pub csrf: &'a str,
    /// The toolbar's context (this hub's groups, the layout).
    pub context: &'a ListContext<'a>,
    pub body: &'a TableBody<'a>,
    pub counts_route: &'a str,
    /// The scope switcher, when the hub has other machines.
    pub scopes: Option<Vec<super::scope::ScopeChoice>>,
    /// Offer New container / pod: this hub's own machine is in scope (new
    /// units are made here; placing them on other machines comes later).
    pub creates: bool,
    pub health: Health,
}

pub fn services_page(p: &ServicesPage) -> Markup {
    let create = html! {
        @if p.creates {
            a.btn.btn-sm.btn-primary href="/containers/new" { (icon(Icon::Plus)) span { "Container" } }
            a.btn.btn-sm href="/pods/new" { (icon(Icon::Plus)) span { "Pod" } }
        }
    };
    let body = html! {
        (super::page_header("Services", html! {}))
        @if let Some(choices) = &p.scopes { (super::scope::scope_switch(choices)) }
        div id="services-counts" hx-get=(p.counts_route)
            hx-trigger="sse:units-changed, sse:any-status delay:300ms, sse:machines-changed" hx-swap="innerHTML" {
            (stats_bar(p.stats))
        }
        (super::list::list_table_with(&SPEC, p.csrf, p.context, create, p.body))
        (bulk_bar(p.csrf))
    };
    shell("Services", Some(NavItem::Services), Some(p.health), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_header_says_whether_its_rows_are_current() {
        let live = machine_row(
            &MachineHeader {
                id: "pi",
                name: "Pi",
                local: false,
                state: HeaderState::Live { count: 1 },
            },
            6,
            "/services/rows",
        )
        .into_string();
        assert!(live.contains("1 service"));
        assert!(live.contains("machine-dot is-online"));
        assert!(!live.contains("Retry"));

        let stale = machine_row(
            &MachineHeader {
                id: "pi",
                name: "Pi",
                local: false,
                state: HeaderState::Stale {
                    count: 3,
                    ago: "5m ago".into(),
                    reason: "no answer within 4 seconds",
                },
            },
            6,
            "/services/rows",
        )
        .into_string();
        assert!(stale.contains("3 services"));
        assert!(stale.contains("Showing what it reported 5m ago"));
        assert!(stale.contains(r#"hx-get="/services/rows""#));

        let never = machine_row(
            &MachineHeader {
                id: "pi",
                name: "Pi",
                local: false,
                state: HeaderState::Unreachable { reason: "refused" },
            },
            6,
            "/services/rows",
        )
        .into_string();
        assert!(never.contains("hasn't answered since the hub started"));
    }

    #[test]
    fn bulk_result_counts_and_lists_failures() {
        let html = bulk_result(
            2,
            &[("radarr.container".into(), "boom".into())],
            "Restarted",
        )
        .into_string();
        assert!(html.contains("Restarted 2 units."));
        assert!(html.contains("<strong>radarr.container</strong>: boom"));
        assert!(
            bulk_result(1, &[], "Stopped")
                .into_string()
                .contains("Stopped 1 unit.")
        );
    }
}
