//! The Settings page's "Pinned image tags" status line (see
//! `crate::imagetags`), plus the list rows' "newer tag" badge. The card's
//! editable fields live in the main settings form, like the self-update
//! card's (`templates::settings::page`).

use maud::{Markup, html};

use super::selfupdate::{ago, csrf_vals};
use super::{BannerKind, banner};
use crate::imagetags::TagsStatus;

/// Re-rendered on `sse:image-tags-changed`, the image-tag analogue of
/// `selfupdate::status_fragment`.
pub fn status_fragment(status: &TagsStatus, csrf: &str) -> Markup {
    html! {
        div #image-tags-status
            hx-get="/settings/image-tags" hx-trigger="sse:image-tags-changed delay:300ms" hx-swap="outerHTML" {
            div.selfupdate-status {
                @if status.checking {
                    p.field-hint { "Checking registries…" }
                } @else if let Some(at) = status.checked_at {
                    p.field-hint {
                        (count(status.repos, "pinned repository", "pinned repositories"))
                        " checked " (ago(at)) "; "
                        (count(status.outdated, "image has", "images have")) " a newer tag."
                    }
                } @else {
                    p.field-hint { "Not checked yet." }
                }
                @if !status.checking && !status.errors.is_empty() {
                    (banner(
                        BannerKind::Warn,
                        &format!("{} couldn't be checked.", count(status.errors.len(), "image", "images")),
                    ))
                    ul.image-tags-errors {
                        @for (image, msg) in &status.errors {
                            li { code { (image) } ": " (msg) }
                        }
                    }
                }
            }
            @if !status.checking {
                div.selfupdate-actions {
                    // A plain button for the same nested-form reason as the
                    // self-update card's (see `selfupdate::status_line`).
                    button.btn.btn-sm type="button"
                        hx-post="/settings/image-tags/check" hx-swap="none"
                        hx-params="csrf_token" hx-vals=(csrf_vals(csrf)) { "Check now" }
                }
            }
        }
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}
