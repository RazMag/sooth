use maud::{Markup, html};

use super::detail;
use super::list::Column;
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;

fn driver_cell(unit: &QuadletUnit, _status: &UnitStatus) -> Markup {
    html! { (unit.section("Volume").and_then(|s| s.get("Driver")).unwrap_or("—")) }
}

pub const COLUMNS: &[Column] = &[Column {
    header: "Driver",
    cell: driver_cell,
}];

pub fn detail_page(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let driver = unit
        .section("Volume")
        .and_then(|s| s.get("Driver"))
        .unwrap_or("—");
    let summary = detail::summary_rows(&[("Driver", html! { (driver) })]);
    detail::detail_page(unit, status, csrf, Some(summary), None)
}
