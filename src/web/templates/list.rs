//! The generic, kind-parameterized list view shared by every section.
//! Columns differ per kind (see `templates::{volumes,networks,...}`); the row
//! shape, empty state, filter box, and SSE refresh wiring are identical
//! everywhere, so they live here once.

use maud::{Markup, html};

use super::{NavItem, kebab_menu, page_header, shell, status_badge};
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;
use crate::web::core;

/// The DOM id of every list's `<tbody>` -- also the SSE-refresh target and
/// the `data-filter-target` of the filter box. One value everywhere.
pub const ROWS_ID: &str = "unit-rows";

pub struct Column {
    pub header: &'static str,
    pub cell: fn(&QuadletUnit, &UnitStatus) -> Markup,
}

pub struct ListSpec {
    pub title: &'static str,
    pub active_nav: Option<NavItem>,
    pub columns: &'static [Column],
    pub new_href: &'static str,
    pub empty_hint: &'static str,
}

/// A "Kind" column cell, shared by any list that mixes multiple kinds
/// together (the Services home page's Container+Pod list, and the generic
/// all-units fallback's every-kind list).
pub fn kind_cell(unit: &QuadletUnit, _status: &UnitStatus) -> Markup {
    html! { (unit.kind.primary_section()) }
}

fn row(unit: &QuadletUnit, status: &UnitStatus, columns: &[Column], csrf: &str) -> Markup {
    let service = unit.service_name();
    html! {
        tr {
            td {
                a href=(core::unit_url(unit)) { (unit.file_name) }
                @if unit.is_template() {
                    " " span.chip.chip-muted title="Template unit — managed read-only, use the CLI to instantiate it" { "template" }
                }
                @if let Some(desc) = unit.description() {
                    div.cell-secondary { (desc) }
                }
            }
            @for column in columns {
                td { ((column.cell)(unit, status)) }
            }
            td { (status_badge(&service, status)) }
            td { (kebab_menu(unit, status, csrf)) }
        }
    }
}

pub fn list_rows(spec: &ListSpec, units: &[(QuadletUnit, UnitStatus)], csrf: &str) -> Markup {
    html! {
        @if units.is_empty() {
            tr { td colspan=(spec.columns.len() + 3) .empty { (spec.empty_hint) } }
        } @else {
            @for (unit, status) in units {
                (row(unit, status, spec.columns, csrf))
            }
        }
    }
}

/// The filter box + table + SSE-refreshed `<tbody>` -- everything below a
/// page's own header. Split out from `list_page` so a page that needs its
/// own custom header (the Services home page, with its stats bar) can still
/// reuse the table itself.
pub fn list_table(
    spec: &ListSpec,
    units: &[(QuadletUnit, UnitStatus)],
    csrf: &str,
    rows_route: &str,
) -> Markup {
    html! {
        div.toolbar {
            input.input.filter-box type="search" data-filter-target=(ROWS_ID) placeholder="Filter…";
        }
        div.table-wrap {
            table.data-table {
                thead {
                    tr {
                        th { "File" }
                        @for column in spec.columns {
                            th { (column.header) }
                        }
                        th { "Status" }
                        th {}
                    }
                }
                tbody id=(ROWS_ID) hx-get=(rows_route) hx-trigger="sse:units-changed" hx-swap="innerHTML" {
                    (list_rows(spec, units, csrf))
                }
            }
        }
    }
}

pub fn list_page(spec: &ListSpec, units: &[(QuadletUnit, UnitStatus)], csrf: &str) -> Markup {
    let body = html! {
        (page_header(spec.title, html! {
            a.btn.btn-primary href=(spec.new_href) { (super::icon(super::Icon::Plus)) span { "New" } }
        }))
        (list_table(spec, units, csrf, &rows_route(spec)))
    };
    shell(spec.title, spec.active_nav, body)
}

/// The `/rows` fragment route lives at `{new_href's section}/rows` -- derived
/// from `new_href` (e.g. `/volumes/new` -> `/volumes/rows`) so a `ListSpec`
/// only needs to state its `new_href` once.
fn rows_route(spec: &ListSpec) -> String {
    let section = spec
        .new_href
        .rsplit_once('/')
        .map(|(prefix, _)| prefix)
        .unwrap_or(spec.new_href);
    format!("{section}/rows")
}
