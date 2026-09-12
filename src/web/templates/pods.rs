use maud::{Markup, html};

use super::{Icon, detail, icon};
use crate::health::Health;
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;

pub fn detail_page(
    unit: &QuadletUnit,
    status: &UnitStatus,
    csrf: &str,
    member_count: usize,
    known_groups: &[String],
    health: Health,
) -> Markup {
    let extra = html! {
        p.detail-links {
            a.btn.btn-ghost.btn-sm href={"/containers/new?pod=" (unit.file_name)} {
                (icon(Icon::Plus)) span { "Add container to this pod" }
            }
        }
    };
    detail::detail_page(
        unit,
        status,
        csrf,
        &[
            ("Members", html! { (member_count) }),
            ("Ports", super::ports_cell_live(unit, status)),
        ],
        Some(extra),
        known_groups,
        health,
    )
}
