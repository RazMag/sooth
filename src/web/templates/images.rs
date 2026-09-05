use maud::{Markup, html};

use super::detail;
use super::list::Column;
use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;

fn type_cell(unit: &QuadletUnit, _status: &UnitStatus) -> Markup {
    html! { (unit.kind.primary_section()) }
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

fn source_cell(unit: &QuadletUnit, _status: &UnitStatus) -> Markup {
    html! { (image_source(unit)) }
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

pub fn detail_page(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let summary = detail::summary_rows(&[
        ("Type", html! { (unit.kind.primary_section()) }),
        ("Source", html! { code { (image_source(unit)) } }),
    ]);
    detail::detail_page(unit, status, csrf, Some(summary), None)
}
