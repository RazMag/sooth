use maud::{Markup, html};

use super::detail::{self, DetailCtx};
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;

/// `secrets`: each `Secret=` name the unit references, with whether podman
/// has it (`None` when the store couldn't be listed).
pub fn detail_page(
    unit: &QuadletUnit,
    status: &UnitStatus,
    csrf: &str,
    secrets: &[(String, Option<bool>)],
    ctx: &DetailCtx,
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
            ("Secrets", secrets_cell(secrets)),
        ],
        None,
        ctx,
    )
}

/// Links each referenced secret to its row on the Secrets page, flagging any
/// podman doesn't have -- the unit can't start until it's set.
fn secrets_cell(secrets: &[(String, Option<bool>)]) -> Markup {
    html! {
        @if secrets.is_empty() {
            span.muted { "—" }
        } @else {
            @for (i, (name, present)) in secrets.iter().enumerate() {
                @if i > 0 { ", " }
                a href={"/secrets#secret-" (name)} { (name) }
                @if *present == Some(false) {
                    " " span.badge.badge-warn title="Not in podman's secret store — set it on the Secrets page" { "missing" }
                }
            }
        }
    }
}
