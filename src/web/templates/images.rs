use maud::{Markup, html};

use super::detail::{self, DetailCtx};
use super::list::{Column, RowCtx};
use crate::quadlet::{QuadletUnit, UnitKind, refs};
use crate::systemd::UnitStatus;

fn image_source(unit: &QuadletUnit) -> &str {
    match unit.kind {
        UnitKind::Image => unit
            .section("Image")
            .and_then(|s| s.get("Image"))
            .unwrap_or("—"),
        UnitKind::Build => unit
            .section("Build")
            .and_then(|s| s.get("ImageTag").or_else(|| s.get("File")))
            .unwrap_or("(build)"),
        _ => "—",
    }
}

fn source_cell(ctx: &RowCtx) -> Markup {
    html! { code.cell-code { (super::soft_wrap_path(image_source(ctx.unit))) } }
}

fn used_by_cell(ctx: &RowCtx) -> Markup {
    super::unit_links(ctx.all_units, &refs::consumers_of(ctx.unit, ctx.all_units))
}

// Image vs Build reads off each row's kind dot -- no "Type" column.
pub const COLUMNS: &[Column] = &[
    Column {
        header: "Source",
        cell: source_cell,
    },
    Column {
        header: "Used by",
        cell: used_by_cell,
    },
];

pub fn detail_page(unit: &QuadletUnit, status: &UnitStatus, csrf: &str, ctx: &DetailCtx) -> Markup {
    // "Type" would duplicate the Overview's "Kind" row -- just Source here.
    detail::detail_page(
        unit,
        status,
        csrf,
        &[("Source", html! { code { (image_source(unit)) } })],
        None,
        ctx,
    )
}
