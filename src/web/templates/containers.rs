use maud::{Markup, html};

use super::detail;
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;

pub fn detail_page(
    unit: &QuadletUnit,
    status: &UnitStatus,
    csrf: &str,
    known_groups: &[String],
) -> Markup {
    let image = unit
        .section("Container")
        .and_then(|s| s.get("Image"))
        .unwrap_or("—");
    detail::detail_page(
        unit,
        status,
        csrf,
        &[
            ("Image", html! { code { (image) } }),
            ("Ports", super::ports_cell_live(unit, status)),
        ],
        None,
        known_groups,
    )
}
