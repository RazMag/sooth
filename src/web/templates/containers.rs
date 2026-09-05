use maud::{Markup, html};

use super::detail;
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;

pub fn detail_page(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let image = unit
        .section("Container")
        .and_then(|s| s.get("Image"))
        .unwrap_or("—");
    let summary = detail::summary_rows(&[("Image", html! { code { (image) } })]);
    detail::detail_page(unit, status, csrf, Some(summary), None)
}
