use maud::{Markup, html};

use super::detail;
use super::list::{Column, RowCtx};
use crate::health::Health;
use crate::quadlet::QuadletUnit;
use crate::quadlet::refs;
use crate::systemd::UnitStatus;

fn driver_cell(ctx: &RowCtx) -> Markup {
    html! { (ctx.unit.section("Volume").and_then(|s| s.get("Driver")).unwrap_or("—")) }
}

fn used_by_cell(ctx: &RowCtx) -> Markup {
    super::unit_links(ctx.all_units, &refs::consumers_of(ctx.unit, ctx.all_units))
}

pub const COLUMNS: &[Column] = &[
    Column {
        header: "Driver",
        cell: driver_cell,
    },
    Column {
        header: "Used by",
        cell: used_by_cell,
    },
];

pub fn detail_page(
    unit: &QuadletUnit,
    status: &UnitStatus,
    csrf: &str,
    all_units: &[QuadletUnit],
    used_by: &[String],
    known_groups: &[String],
    health: Health,
) -> Markup {
    let driver = unit
        .section("Volume")
        .and_then(|s| s.get("Driver"))
        .unwrap_or("—");
    detail::detail_page(
        unit,
        status,
        csrf,
        &[
            ("Driver", html! { (driver) }),
            ("Used by", super::unit_links(all_units, used_by)),
        ],
        None,
        known_groups,
        health,
    )
}
