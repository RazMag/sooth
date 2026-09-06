//! One logs page template, reused by every section (the log stream is
//! kind-agnostic -- it's just `journalctl` against the unit's service name).
//! The live tail is wired up by `frontend/logs.js` off the `data-log-stream`
//! attribute; there is no page-specific inline script.

use maud::{Markup, html};

use super::{NavItem, back_link, page_header, shell};
use crate::quadlet::QuadletUnit;
use crate::web::core;

pub fn logs_page(unit: &QuadletUnit, initial: &str) -> Markup {
    let service = unit.service_name();
    let stream_url = format!("{}/logs/stream", core::unit_url(unit));
    let body = html! {
        (back_link(&core::unit_url(unit), &unit.file_name))
        (page_header(&format!("Logs: {}", unit.file_name), html! {}))
        p.page-meta { code { (service) } }
        pre id="log-output" data-log-stream data-stream-url=(stream_url) { (initial) }
    };
    shell(
        &format!("Logs: {}", unit.file_name),
        NavItem::for_kind(unit.kind),
        body,
    )
}
