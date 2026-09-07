use maud::{Markup, html};

use super::detail;
use super::list::{Column, RowCtx};
use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;

fn type_cell(ctx: &RowCtx) -> Markup {
    html! { (ctx.unit.kind.primary_section()) }
}

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
    html! { (image_source(ctx.unit)) }
}

pub const COLUMNS: &[Column] = &[
    Column {
        header: "Type",
        cell: type_cell,
    },
    Column {
        header: "Source",
        cell: source_cell,
    },
];

pub fn detail_page(
    unit: &QuadletUnit,
    status: &UnitStatus,
    csrf: &str,
    known_groups: &[String],
) -> Markup {
    // "Type" would duplicate the Overview's "Kind" row -- just Source here.
    detail::detail_page(
        unit,
        status,
        csrf,
        &[("Source", html! { code { (image_source(unit)) } })],
        None,
        known_groups,
    )
}
