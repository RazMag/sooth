//! The Services home page: the actual running workloads (Containers + Pods)
//! combined into one list, with a stats bar on top. Volumes/Networks/Images/
//! Ports are resource sections, not "services" -- they stay their own sidebar
//! entries, one click away, rather than cluttering this page.

use maud::{Markup, html};

use super::list::{Column, ListSpec, RowCtx, kind_cell};
use super::{Icon, NavItem, icon, ports_summary, shell};
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;

fn ports_cell(ctx: &RowCtx) -> Markup {
    ports_summary(ctx.unit, ctx.status.is_active())
}

pub const COLUMNS: &[Column] = &[
    Column {
        header: "Kind",
        cell: kind_cell,
    },
    Column {
        header: "Ports",
        cell: ports_cell,
    },
];

pub const SPEC: ListSpec = ListSpec {
    title: "Services",
    active_nav: Some(NavItem::Services),
    columns: COLUMNS,
    new_href: "/containers/new",
    empty_hint: "No services yet — create a container or pod to get started.",
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

pub fn services_page(
    units: &[(QuadletUnit, UnitStatus)],
    stats: &Stats,
    csrf: &str,
    all_units: &[QuadletUnit],
) -> Markup {
    let body = html! {
        (super::page_header("Services", html! {
            a.btn.btn-primary href="/containers/new" { (icon(Icon::Plus)) span { "Container" } }
            a.btn href="/pods/new" { (icon(Icon::Plus)) span { "Pod" } }
        }))
        div id="services-counts" hx-get="/services/counts"
            hx-trigger="sse:units-changed, sse:any-status delay:300ms" hx-swap="innerHTML" {
            (stats_bar(stats))
        }
        (super::list::list_table(&SPEC, units, csrf, "/services/rows", all_units))
    };
    shell("Services", Some(NavItem::Services), body)
}
