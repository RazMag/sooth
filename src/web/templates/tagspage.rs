//! The Images & tags page (`/tags`): every pinned container that has a
//! newer tag, with the same Switch menu its Services row offers, and the
//! tag check's status. "Review tags" on the Overview lands here.

use maud::{Markup, html};

use super::imagetags::{BadgeNote, status_fragment, tag_badge, tag_slot};
use super::{NavItem, page_header, shell, soft_wrap_path};
use crate::health::Health;
use crate::imagetags::TagsStatus;
use crate::web::core::NewerTags;

/// One pinned container with newer tags.
pub struct TagRow {
    pub file_name: String,
    pub href: String,
    pub image: String,
    pub tags: NewerTags,
}

pub fn tags_page(rows: &[TagRow], status: &TagsStatus, csrf: &str, health: Health) -> Markup {
    let body = html! {
        (page_header("Images & tags", html! {}))
        p.page-meta {
            "Containers pinned to a version tag, checked against their registry for newer ones. "
            "Switching rewrites the tag in the quadlet, pulls it and restarts the container if it's running."
        }
        div.card.tags-status { (status_fragment(status, csrf)) }
        @if rows.is_empty() {
            p.empty { "Every pinned image is on its newest tag." }
        } @else {
            div.table-wrap {
                table.data-table {
                    thead { tr { th { "Container" } th { "Image" } th { "Pinned" } th { "Newer" } } }
                    tbody id="tag-rows" hx-get="/tags/rows"
                        hx-trigger="sse:image-tags-changed delay:300ms, sse:units-changed delay:300ms"
                        hx-swap="innerHTML" {
                        (tag_rows(rows, csrf))
                    }
                }
            }
        }
    };
    shell(
        "Images & tags",
        Some(NavItem::ImageTags),
        Some(health),
        body,
    )
}

/// The table rows -- also `GET /tags/rows`.
pub fn tag_rows(rows: &[TagRow], csrf: &str) -> Markup {
    html! {
        @for r in rows {
            tr {
                td { a href=(r.href) { (r.file_name) } }
                td { code.cell-code { (soft_wrap_path(&r.image)) } }
                td { code { (r.tags.current) } }
                td { (tag_slot(&r.file_name, tag_badge(&r.href, &r.file_name, &r.tags, BadgeNote::None, csrf), false)) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_page_says_everything_is_current() {
        let html = tags_page(&[], &TagsStatus::default(), "c", Health::default()).into_string();
        assert!(html.contains("Every pinned image is on its newest tag."));
        assert!(html.contains(r#"href="/tags" aria-current="page""#));
    }
}
