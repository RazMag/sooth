//! The generic, kind-parameterized list view shared by every section.
//! Columns differ per kind (see `templates::{volumes,networks,...}`); the row
//! shape, empty state, filter box, and SSE refresh wiring are identical
//! everywhere, so they live here once.

use maud::{Markup, html};

use super::{NavItem, autostart_pill, kebab_menu, page_header, shell, status_badge};
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;
use crate::web::core;

/// The DOM id of every list's `<tbody>` -- also the SSE-refresh target and
/// the `data-filter-target` of the filter box. One value everywhere.
pub const ROWS_ID: &str = "unit-rows";

/// What a column cell can draw on: the row's own unit and live status, plus
/// every quadlet on disk (all kinds) for columns that resolve cross-unit
/// references -- the Volumes/Networks "Used by" column. `all_units` is an
/// empty slice when the caller didn't supply siblings.
pub struct RowCtx<'a> {
    pub unit: &'a QuadletUnit,
    pub status: &'a UnitStatus,
    pub all_units: &'a [QuadletUnit],
}

pub struct Column {
    pub header: &'static str,
    pub cell: fn(&RowCtx) -> Markup,
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
pub fn kind_cell(ctx: &RowCtx) -> Markup {
    html! { (ctx.unit.kind.primary_section()) }
}

fn row(
    unit: &QuadletUnit,
    status: &UnitStatus,
    columns: &[Column],
    csrf: &str,
    all_units: &[QuadletUnit],
) -> Markup {
    let service = unit.service_name();
    let ctx = RowCtx {
        unit,
        status,
        all_units,
    };
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
                td { ((column.cell)(&ctx)) }
            }
            td { (status_badge(&service, status)) (autostart_pill(status.is_autostart_enabled())) }
            td { (kebab_menu(unit, status, csrf)) }
        }
    }
}

pub fn list_rows(
    spec: &ListSpec,
    units: &[(QuadletUnit, UnitStatus)],
    csrf: &str,
    all_units: &[QuadletUnit],
) -> Markup {
    html! {
        @if units.is_empty() {
            tr { td colspan=(spec.columns.len() + 3) .empty { (spec.empty_hint) } }
        } @else {
            @for (unit, status) in units {
                (row(unit, status, spec.columns, csrf, all_units))
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
    all_units: &[QuadletUnit],
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
                // Only a create/edit/delete rebuilds the whole row set. A
                // status change updates each row's badge (its own `sse-swap`)
                // and the kebab's action forms (a scoped swap inside the
                // menu) in place -- swapping the whole `<tbody>` here would
                // slam shut any menu the user has open.
                tbody id=(ROWS_ID) hx-get=(rows_route) hx-trigger="sse:units-changed" hx-swap="innerHTML" {
                    (list_rows(spec, units, csrf, all_units))
                }
            }
        }
    }
}

pub fn list_page(
    spec: &ListSpec,
    units: &[(QuadletUnit, UnitStatus)],
    csrf: &str,
    all_units: &[QuadletUnit],
) -> Markup {
    let body = html! {
        (page_header(spec.title, html! {
            a.btn.btn-primary href=(spec.new_href) { (super::icon(super::Icon::Plus)) span { "New" } }
        }))
        (list_table(spec, units, csrf, &rows_route(spec), all_units))
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
