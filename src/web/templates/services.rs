//! The Services home page: the actual running workloads (Containers + Pods)
//! combined into one list, with a stats bar on top. Volumes/Networks/Images/
//! Ports are resource sections, not "services" -- they stay their own sidebar
//! entries, one click away, rather than cluttering this page.

use maud::{Markup, html};

use super::list::{Column, ListContext, ListSpec, RowCtx};
use super::{Icon, NavItem, icon, ports_cell_live, shell, split_ports_cell_live};
use crate::health::Health;
use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;

/// A pod's published ports show on the member(s) serving them, the rest on
/// the pod itself (see `ListContext::ports`); any other row shows its own.
fn ports_cell(ctx: &RowCtx) -> Markup {
    match ctx.split_ports {
        Some(split) => split_ports_cell_live(ctx.unit, ctx.status, split),
        None => ports_cell_live(ctx.unit, ctx.status),
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

pub fn services_page(
    units: &[(QuadletUnit, UnitStatus)],
    stats: &Stats,
    csrf: &str,
    all_units: &[QuadletUnit],
    groups: &ListContext,
    health: Health,
) -> Markup {
    let create = html! {
        a.btn.btn-sm.btn-primary href="/containers/new" { (icon(Icon::Plus)) span { "Container" } }
        a.btn.btn-sm href="/pods/new" { (icon(Icon::Plus)) span { "Pod" } }
    };
    let body = html! {
        (super::page_header("Services", html! {}))
        div id="services-counts" hx-get="/services/counts"
            hx-trigger="sse:units-changed, sse:any-status delay:300ms" hx-swap="innerHTML" {
            (stats_bar(stats))
        }
        (super::list::list_table(&SPEC, units, csrf, "/services/rows", all_units, groups, create))
        (bulk_bar(csrf))
    };
    shell("Services", Some(NavItem::Services), Some(health), body)
}

#[cfg(test)]
mod tests {
    use super::*;

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
