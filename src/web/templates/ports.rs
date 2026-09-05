use maud::{Markup, html};

use super::{NavItem, page_header, shell, status_badge};
use crate::quadlet::naming;
use crate::quadlet::ports::PortMapping;
use crate::systemd::UnitStatus;

/// One rendered row, fully resolved by the handler (owner URL, live status,
/// whether it's part of a flagged collision) so the template does no lookups
/// of its own.
pub struct PortRow<'a> {
    pub mapping: &'a PortMapping,
    pub owner_href: String,
    pub status: &'a UnitStatus,
    pub collides: bool,
}

pub fn ports_page(rows: &[PortRow]) -> Markup {
    let body = html! {
        (page_header("Ports", html! {}))
        p.page-meta { "Declared " code { "PublishPort=" } " entries across Containers and Pods." }
        @if rows.is_empty() {
            p.empty { "No published ports declared yet." }
        } @else {
            div.table-wrap {
                table.data-table {
                    thead {
                        tr {
                            th { "Host Port" }
                            th { "Protocol" }
                            th { "Container Port" }
                            th { "Owner" }
                            th { "Status" }
                        }
                    }
                    tbody {
                        @for row in rows {
                            tr class=[row.collides.then_some("row-collision")] {
                                td title=(row.mapping.raw) {
                                    @match row.mapping.host_port {
                                        Some(r) if r.start == r.end => (r.start.to_string()),
                                        Some(r) => (format!("{}-{}", r.start, r.end)),
                                        None => span.muted { "dynamic" },
                                    }
                                    @if row.collides {
                                        div.cell-secondary { "conflicts with another unit" }
                                    }
                                }
                                td { (row.mapping.protocol.as_str()) }
                                td { (row.mapping.container_port) }
                                td { a href=(row.owner_href) { (row.mapping.file_name) } }
                                td { (status_badge(&naming::service_name(&row.mapping.file_name), row.status)) }
                            }
                        }
                    }
                }
            }
        }
    };
    shell("Ports", Some(NavItem::Ports), body)
}
