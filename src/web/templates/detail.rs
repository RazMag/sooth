//! The one detail-page skeleton, shared by every kind. Each kind's own
//! module supplies just its `summary` (a small key/value card) and, for
//! pods, an `extra` block; the header, status, action row, Edit/Logs/Delete
//! links, and the verbatim "Parsed contents" section tables are identical
//! everywhere and live here.

use maud::{Markup, html};

use super::{
    BannerKind, NavItem, action_row, banner, detail_links, page_header, section_table, shell,
    status_badge,
};
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;
use crate::web::core;

pub fn detail_page(
    unit: &QuadletUnit,
    status: &UnitStatus,
    csrf: &str,
    summary: Option<Markup>,
    extra: Option<Markup>,
) -> Markup {
    let service = unit.service_name();
    let base = core::unit_url(unit);
    let active = NavItem::for_kind(unit.kind);
    let body = html! {
        (page_header(&unit.file_name, status_badge(&service, status)))
        p.page-meta { code { (unit.path.display().to_string()) } " · " (unit.kind.primary_section()) }

        @if unit.is_template() {
            (banner(BannerKind::Info, "Template unit — managed read-only. Use the CLI to instantiate it."))
        } @else {
            (action_row(unit, status, csrf))
        }

        (detail_links(&base, csrf))

        @if let Some(s) = summary {
            div.card { (s) }
        }
        @if let Some(e) = extra { (e) }

        h2 { "Parsed contents" }
        (section_table(unit))
    };
    shell(&unit.file_name, active, body)
}

/// A one-column key/value card body, the common shape of a kind's `summary`.
pub fn summary_rows(rows: &[(&str, Markup)]) -> Markup {
    html! {
        table.kv-table {
            @for (key, value) in rows {
                tr { td { (key) } td { (value) } }
            }
        }
    }
}
