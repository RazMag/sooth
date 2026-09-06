use maud::{Markup, html};

use super::detail;
use super::list::Column;
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;

fn driver_cell(unit: &QuadletUnit, _status: &UnitStatus) -> Markup {
    html! { (unit.section("Network").and_then(|s| s.get("Driver")).unwrap_or("—")) }
}

fn subnet_cell(unit: &QuadletUnit, _status: &UnitStatus) -> Markup {
    html! { (unit.section("Network").and_then(|s| s.get("Subnet")).unwrap_or("—")) }
}

pub const COLUMNS: &[Column] = &[
    Column {
        header: "Driver",
        cell: driver_cell,
    },
    Column {
        header: "Subnet",
        cell: subnet_cell,
    },
];

pub fn detail_page(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let driver = unit
        .section("Network")
        .and_then(|s| s.get("Driver"))
        .unwrap_or("—");
    let subnet = unit
        .section("Network")
        .and_then(|s| s.get("Subnet"))
        .unwrap_or("—");
    detail::detail_page(
        unit,
        status,
        csrf,
        &[
            ("Driver", html! { (driver) }),
            ("Subnet", html! { code { (subnet) } }),
        ],
        None,
    )
}
