//! The one detail-page skeleton, shared by every kind. Each kind's own
//! module supplies just its `facts` (extra key/value rows for the Overview
//! rail) and, for pods, an `extra` block; the header, status, action
//! toolbar, two-column layout, and the verbatim `[Section]` config cards are
//! identical everywhere and live here.

use maud::{Markup, html};

use super::{
    BannerKind, NavItem, action_row, back_link, banner, detail_links, group_picker, page_header,
    section_back_target, section_table, shell, status_badge,
};
use crate::health::Health;
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;
use crate::web::core;

pub fn detail_page(
    unit: &QuadletUnit,
    status: &UnitStatus,
    csrf: &str,
    facts: &[(&str, Markup)],
    extra: Option<Markup>,
    known_groups: &[String],
    health: Health,
) -> Markup {
    let service = unit.service_name();
    let base = core::unit_url(unit);
    let active = NavItem::for_kind(unit.kind);
    let (back_href, back_label) = section_back_target(unit.kind);
    let body = html! {
        (back_link(back_href, back_label))
        (page_header(&unit.file_name, status_badge(&service, status)))

        div.detail-toolbar {
            // Which buttons to show depends on live state, so re-fetch this
            // slot whenever *this* unit's status changes -- the same SSE
            // event the badge listens to. The `/actions` fragment renders
            // the same markup.
            div.detail-actions-live hx-get={(base) "/actions"} hx-trigger={"sse:status-" (service) " delay:300ms"} hx-swap="innerHTML" {
                @if unit.is_template() {
                    (banner(BannerKind::Info, "Template unit — managed read-only. Use the CLI to instantiate it."))
                } @else {
                    (action_row(unit, status, csrf))
                }
            }
            (detail_links(&base, csrf))
        }

        div.detail-grid {
            aside.detail-aside {
                div.card {
                    h2.card-title { "Overview" }
                    table.kv-table {
                        tr { td { "Kind" } td { (unit.kind.primary_section()) } }
                        tr { td { "Service" } td { code { (service) } } }
                        tr { td { "File" } td { code { (unit.path.display().to_string()) } } }
                        @if !unit.is_template() {
                            tr {
                                td { "Group" }
                                td { (group_picker(&base, &unit.group, csrf, known_groups)) }
                            }
                        }
                        @for (key, value) in facts {
                            tr { td { (key) } td { (value) } }
                        }
                    }
                }
                @if let Some(e) = extra { (e) }
            }
            div.detail-main {
                h2 { "Configuration" }
                // The quadlet file changes under an open detail page on an
                // autostart toggle (Enable/Disable patches the `[Install]`
                // section) or an external edit, so re-fetch the rendered
                // sections on the same broadcast the list pages react to.
                div.config-live hx-get={(base) "/config"} hx-trigger="sse:units-changed delay:300ms" hx-swap="innerHTML" {
                    (section_table(unit))
                }
            }
        }
    };
    shell(&unit.file_name, active, Some(health), body)
}
