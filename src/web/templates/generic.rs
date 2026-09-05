//! Templates for the generic `/units` fallback section: a kind-agnostic
//! detail page (used for Kube units, which have no dedicated section) and the
//! shared raw-textarea create/edit forms used by every section (Volumes,
//! Networks, Images) plus this fallback itself.

use maud::{Markup, html};

use super::detail;
use super::{BannerKind, NavItem, banner, code_editor, csrf_input, page_header, shell};
use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;

/// A plain, kind-branch-free detail page -- used for any kind without a
/// dedicated section template (today, just Kube).
pub fn detail_page(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    detail::detail_page(unit, status, csrf, None, None)
}

/// Shared by every section's "New" page -- the only differences between, say,
/// `/containers/new` and `/volumes/new` are the POST target, nav highlight,
/// and starter skeleton passed in here. `action` and `active` are
/// independent: Containers and Pods both highlight Services but post to
/// different URLs.
pub fn new_unit_page(
    csrf: &str,
    active: Option<NavItem>,
    action: &str,
    skeleton: &str,
    error: Option<&str>,
) -> Markup {
    let body = html! {
        (page_header("New quadlet", html! {}))
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }
        form method="post" action=(action) {
            (csrf_input(csrf))
            div.field {
                label for="file_name" { "File name" }
                input.input type="text" id="file_name" name="file_name" placeholder="myapp.container" required;
                @if active.is_none() {
                    span.field-hint { "Valid extensions: " (kind_names().join(", ")) }
                }
            }
            div.field {
                label { "Contents" }
                (code_editor(skeleton, Some("#file_name"), None))
            }
            button.btn.btn-primary type="submit" { "Create" }
        }
    };
    shell("New quadlet", active, body)
}

pub fn edit_unit_page(unit: &QuadletUnit, csrf: &str, error: Option<&str>) -> Markup {
    let base = crate::web::core::unit_url(unit);
    let body = html! {
        (page_header(&format!("Edit {}", unit.file_name), html! {}))
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }
        form method="post" action=(base + "/edit") {
            (csrf_input(csrf))
            div.field {
                label { "Contents" }
                (code_editor(&unit.raw, None, Some(&unit.file_name)))
            }
            button.btn.btn-primary type="submit" { "Save" }
        }
    };
    shell(
        &format!("Edit {}", unit.file_name),
        NavItem::for_kind(unit.kind),
        body,
    )
}

fn kind_names() -> Vec<&'static str> {
    UnitKind::all().iter().map(|k| k.extension()).collect()
}
