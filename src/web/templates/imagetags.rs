//! The Settings page's "Pinned image tags" status line (see
//! `crate::imagetags`), plus the list rows' "newer tag" badge. The card's
//! editable fields live in the main settings form, like the self-update
//! card's (`templates::settings::page`).

use maud::{Markup, html};

use super::selfupdate::{ago, csrf_vals};
use super::{BannerKind, banner, csrf_input};
use crate::imagetags::TagsStatus;
use crate::web::core::{NewerTags, TagSwitch};

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
                        (count(status.outdated, "container has", "containers have")) " a newer tag."
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

/// What the badge's panel shows below its tags, after a switch attempt.
pub enum BadgeNote<'a> {
    None,
    Failed(&'a str),
    Switched(&'a TagSwitch),
}

/// The selector-safe id of a container row's badge slot, so a switch or
/// the kebab's check can swap the badge in place.
fn slot_id(file_name: &str) -> String {
    let safe: String = file_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("tags-{safe}")
}

/// Every container row's (possibly empty) slot for its [`tag_badge`]. `oob`
/// marks it for an htmx out-of-band swap -- how the kebab's "Check for
/// update" brings a badge in without redrawing the row (which would close
/// the open menu).
pub fn tag_slot(file_name: &str, badge: Markup, oob: bool) -> Markup {
    html! {
        span.tag-slot id=(slot_id(file_name)) hx-swap-oob=[oob.then_some("innerHTML")] { (badge) }
    }
}

/// A pinned container's "newer tag" badge on its list row: the newest tag
/// as a pill that opens a small menu (`details.menu`, positioned by
/// `menu.js`) of every newer tag worth offering, each a "Switch" button
/// (`POST {unit}/image-tag` -> `core::switch_image_tag`). The response is
/// this badge again, swapped into its [`tag_slot`] with the outcome under
/// the tags; the switch's own `units-changed` then redraws the row.
pub fn tag_badge(
    base: &str,
    file_name: &str,
    tags: &NewerTags,
    note: BadgeNote<'_>,
    csrf: &str,
) -> Markup {
    let slot = slot_id(file_name);
    let Some(newest) = tags.candidates.last() else {
        return html! {};
    };
    let title = format!(
        "Pinned to {}; the registry has newer version tags",
        tags.current
    );
    html! {
        details.menu.menu-start.tag-menu open[!matches!(note, BadgeNote::None)] {
            summary.badge.badge-update title=(title) aria-label=(title) { (newest.tag) }
            div.menu-panel {
                div.menu-update {
                    // One span per note: `.menu-note` is a flex row, which
                    // would lay each text run and `<code>` out as its own item.
                    p.menu-note { span { "Pinned to " code { (tags.current) } } }
                    @if let Some((image, users)) = &tags.shared {
                        p.menu-note { span {
                            @if *users > 1 {
                                "Set in " code { (image) } ", which " (users)
                                " containers use -- switching moves all of them."
                            } @else {
                                "Set in " code { (image) } "."
                            }
                        } }
                    }
                    @for c in &tags.candidates {
                        form hx-post={(base) "/image-tag"} hx-target={"#" (slot)} hx-swap="innerHTML"
                            hx-disabled-elt="find button" {
                            (csrf_input(csrf))
                            input type="hidden" name="tag" value=(c.tag);
                            button.btn.btn-sm.btn-enable.tag-menu-switch type="submit" disabled[tags.blocked.is_some()]
                                title="Pull this tag, switch Image= to it, and restart the container if it's running" {
                                span.menu-idle { "Switch to " (c.tag) }
                                span.menu-busy { "Switching…" }
                                small.tag-scope {
                                    @match &c.within {
                                        Some(range) => { "newest " (range) }
                                        None => "newest",
                                    }
                                }
                            }
                        }
                    }
                    @if let Some(why) = &tags.blocked {
                        p.menu-note { (why) }
                    }
                    @match note {
                        BadgeNote::None => {}
                        BadgeNote::Failed(msg) => p.menu-note.error { (msg) },
                        BadgeNote::Switched(done) => p.menu-note.ok { span {
                            "Switched to " code { (done.image) }
                            @if done.restarted.is_empty() { "; used on next start." } @else { " and restarted." }
                        } },
                    }
                }
            }
        }
    }
}
