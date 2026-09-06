//! The one detail-page skeleton, shared by every kind. Each kind's own
//! module supplies just its `summary` (a small key/value card) and, for
//! pods, an `extra` block; the header, status, action row, Edit/Logs/Delete
//! links, and the verbatim "Parsed contents" section tables are identical
//! everywhere and live here.

use maud::{Markup, html};

use super::{
    BannerKind, NavItem, action_row, back_link, banner, detail_links, page_header,
    section_back_target, section_table, shell, status_badge,
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
    let (back_href, back_label) = section_back_target(unit.kind);
    let body = html! {
        (back_link(back_href, back_label))
        (page_header(&unit.file_name, status_badge(&service, status)))
        p.page-meta { code { (unit.path.display().to_string()) } " · " (unit.kind.primary_section()) }

        // Which buttons to show depends on live state, so re-fetch this slot
        // whenever *this* unit's status changes -- the same SSE event the
        // badge listens to. The `/actions` fragment renders the same markup.
        div hx-get={(base) "/actions"} hx-trigger={"sse:status-" (service) " delay:300ms"} hx-swap="innerHTML" {
            @if unit.is_template() {
                (banner(BannerKind::Info, "Template unit — managed read-only. Use the CLI to instantiate it."))
            } @else {
                (action_row(unit, status, csrf))
            }
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
