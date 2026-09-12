//! The generic, kind-parameterized list view shared by every section.
//! Columns differ per kind (see `templates::{volumes,networks,...}`); the row
//! shape, empty state, filter box, and SSE refresh wiring are identical
//! everywhere, so they live here once.

use maud::{Markup, html};

use super::{
    Icon, NavItem, autostart_pill, autoupdate_pill, csrf_input, group_kebab, icon, kebab_menu,
    known_groups_datalist, page_header, shell, status_badge,
};
use crate::health::Health;
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;
use crate::web::core;

/// The DOM id of every list's `<tbody>` -- also the SSE-refresh target and
/// the `data-filter-target` of the filter box. One value everywhere.
pub const ROWS_ID: &str = "unit-rows";

/// The two group-name lists every list render needs, bundled into one
/// parameter (keeps `list_table` under clippy's argument-count lint):
/// `known` is every existing group directory (drop target / "move to"
/// autocomplete source), `synced` is the subset of those git-sync already
/// manages -- see `web::core`'s `synced_destination` / `synced_source`,
/// the authoritative version of the same check this only previews for the
/// drag-and-drop guard in `dragdrop.js` and the "synced" chip below.
pub struct GroupLists<'a> {
    pub known: &'a [String],
    pub synced: &'a [String],
}

/// What a column cell can draw on: the row's own unit and live status, plus
/// every quadlet on disk (all kinds) for columns that resolve cross-unit
/// references -- the Volumes/Networks "Used by" column. `all_units` is an
/// empty slice when the caller didn't supply siblings.
pub struct RowCtx<'a> {
    pub unit: &'a QuadletUnit,
    pub status: &'a UnitStatus,
    pub all_units: &'a [QuadletUnit],
}

pub struct Column {
    pub header: &'static str,
    pub cell: fn(&RowCtx) -> Markup,
}

pub struct ListSpec {
    pub title: &'static str,
    pub active_nav: Option<NavItem>,
    pub columns: &'static [Column],
    pub new_href: &'static str,
    pub empty_hint: &'static str,
}

/// A "Kind" column cell, shared by any list that mixes multiple kinds
/// together (the Services home page's Container+Pod list, and the generic
/// all-units fallback's every-kind list).
pub fn kind_cell(ctx: &RowCtx) -> Markup {
    html! { (ctx.unit.kind.primary_section()) }
}

fn row(
    unit: &QuadletUnit,
    status: &UnitStatus,
    columns: &[Column],
    csrf: &str,
    all_units: &[QuadletUnit],
    known_groups: &[String],
    group_member: Option<&str>,
) -> Markup {
    let service = unit.service_name();
    let ctx = RowCtx {
        unit,
        status,
        all_units,
    };
    let move_url = format!("{}/move", core::unit_url(unit));
    // A member of `media/arr` renders one indent step past the "arr" header.
    let depth = group_member.map_or(0, |g| g.matches('/').count() + 1);
    html! {
        tr class=[group_member.map(|_| "is-collapsed")]
            data-group-member=[group_member]
            data-depth=(depth)
            data-move-url=(move_url) {
            td {
                div.row-indent style=(format!("--depth:{depth}")) {
                    @if !unit.is_template() {
                        span.drag-handle draggable="true"
                            title="Drag to file this unit under another group" aria-hidden="true" {
                            (icon(Icon::Grip))
                        }
                    }
                    a href=(core::unit_url(unit)) { (unit.file_name) }
                    @if unit.is_template() {
                        " " span.chip.chip-muted title="Template unit — managed read-only, use the CLI to instantiate it" { "template" }
                    }
                    @if let Some(desc) = unit.description() {
                        div.cell-secondary { (desc) }
                    }
                }
            }
            @for column in columns {
                td { ((column.cell)(&ctx)) }
            }
            td {
                (status_badge(&service, status))
                (autostart_pill(status.is_autostart_enabled()))
                (autoupdate_pill(unit))
            }
            td { (kebab_menu(unit, status, csrf, known_groups)) }
        }
    }
}

/// The full ordered set of group paths to render sections for: every group a
/// listed unit is in, plus every group directory that exists on disk (so a
/// freshly-created but still-empty group shows up as a drop target). Sorted,
/// which puts each parent path immediately before its children.
fn section_groups<'a>(
    units: &'a [(QuadletUnit, UnitStatus)],
    known_groups: &'a [String],
) -> Vec<&'a str> {
    let mut groups: Vec<&str> = units
        .iter()
        .map(|(u, _)| u.group.as_str())
        .filter(|g| !g.is_empty())
        .chain(known_groups.iter().map(String::as_str))
        .collect();
    groups.sort_unstable();
    groups.dedup();
    groups
}

/// How many units live in `group` *or any of its descendant groups*. This is
/// the number a section header shows: collapsing a group hides its subgroups
/// and their rows too, so the count has to speak for everything underneath,
/// not just the units filed directly in that directory.
fn nested_unit_count(units: &[(QuadletUnit, UnitStatus)], group: &str) -> usize {
    units
        .iter()
        .filter(|(u, _)| {
            u.group == group
                || u.group
                    .strip_prefix(group)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .count()
}

/// A collapsible section header row for one group directory. It is both a drop
/// target for "move a unit into this group" and, via its own grip handle, a
/// draggable to re-parent the whole group (see `frontend/dragdrop.js`); the ⋯
/// menu adds a subgroup / renames / moves / deletes it. The row keeps the same
/// column shape as a unit row -- a wide first cell plus a trailing kebab cell
/// -- so its menu lines up with the unit rows' menus. Rendered with
/// `aria-expanded="false"`; `groups.js` reconciles it against the per-browser
/// remembered state, and member rows carry `is-collapsed` so the no-JS /
/// pre-JS view starts collapsed.
fn group_header_row(path: &str, count: usize, colspan: usize, csrf: &str, synced: bool) -> Markup {
    let depth = path.matches('/').count();
    let (parent, last) = match path.rsplit_once('/') {
        Some((p, l)) => (Some(p), l),
        None => (None, path),
    };
    html! {
        tr.group-row data-group=(path) data-depth=(depth) {
            td.group-head-cell colspan=(colspan - 1) {
                div.group-row-inner style=(format!("--depth:{depth}")) {
                    // A synced group's own directory shouldn't be re-parented
                    // by hand -- see `web::core::synced_source` -- so it gets
                    // no drag grip of its own, just the chip below.
                    @if !synced {
                        span.drag-handle.group-drag draggable="true"
                            title="Drag to move this group under another" aria-hidden="true" {
                            (icon(Icon::Grip))
                        }
                    }
                    button.group-toggle type="button" aria-expanded="false" title=(path) {
                        span.group-chevron aria-hidden="true" { (icon(Icon::ChevronDown)) }
                        span.group-name {
                            @if let Some(p) = parent {
                                span.group-parent { (p) "/" }
                            }
                            (last)
                        }
                        span.group-count { (count) }
                    }
                    @if synced {
                        a.chip.chip-synced href="/git-sync"
                            title="Synced from a git repository -- managed by the remote, see the Git Sync page" {
                            (icon(Icon::GitSync)) "synced"
                        }
                    }
                }
            }
            td.group-kebab-cell { (group_kebab(path, csrf)) }
        }
    }
}

pub fn list_rows(
    spec: &ListSpec,
    units: &[(QuadletUnit, UnitStatus)],
    csrf: &str,
    all_units: &[QuadletUnit],
    groups: &GroupLists,
) -> Markup {
    let known_groups = groups.known;
    let sections = section_groups(units, known_groups);
    if units.is_empty() && sections.is_empty() {
        return html! {
            tr { td colspan=(spec.columns.len() + 3) .empty { (spec.empty_hint) } }
        };
    }
    let colspan = spec.columns.len() + 3;
    html! {
        // Root (ungrouped) units first, bare.
        @for (unit, status) in units.iter().filter(|(u, _)| u.group.is_empty()) {
            (row(unit, status, spec.columns, csrf, all_units, known_groups, None))
        }
        // Then one collapsible section per group directory.
        @for grp in sections {
            @let members: Vec<&(QuadletUnit, UnitStatus)> =
                units.iter().filter(|(u, _)| u.group == grp).collect();
            @let nested = nested_unit_count(units, grp);
            @let synced = groups.synced.iter().any(|g| g == grp);
            (group_header_row(grp, nested, colspan, csrf, synced))
            @if members.is_empty() && nested == 0 {
                @let d = grp.matches('/').count() + 1;
                tr.group-empty.is-collapsed data-group-member=(grp) data-depth=(d) {
                    td colspan=(colspan) {
                        div.row-indent style=(format!("--depth:{d}")) {
                            "Empty — drag a unit here to file it under this group."
                        }
                    }
                }
            } @else {
                @for (unit, status) in members {
                    (row(unit, status, spec.columns, csrf, all_units, known_groups, Some(grp)))
                }
            }
        }
    }
}

/// The toolbar's "Add group" disclosure: creates an empty group directory so
/// it can be used as a drag target before anything lives in it. htmx post +
/// `hx-swap="none"`; the SSE `units-changed` refresh redraws the table.
fn add_group_control(csrf: &str) -> Markup {
    html! {
        details.add-group {
            summary.btn.btn-sm { (icon(Icon::Plus)) span { "Add group" } }
            form.add-group-form hx-post="/groups" hx-swap="none" {
                (csrf_input(csrf))
                input.input.input-sm type="text" name="group" placeholder="e.g. media/arr"
                    list="known-groups" aria-label="New group name"
                    autocomplete="off" autocapitalize="off" spellcheck="false" required;
                button.btn.btn-sm.btn-primary type="submit" { "Create" }
            }
        }
    }
}

/// The toolbar's create action(s), right-aligned next to "Add group": a
/// single "New" for the generic sections, "Container" + "Pod" for the
/// Services home page. Same `.btn-sm` scale as the rest of the toolbar so
/// the create controls, the filter box and "Add group" read as one bar.
fn new_actions(spec: &ListSpec) -> Markup {
    html! {
        a.btn.btn-sm.btn-primary href=(spec.new_href) { (icon(Icon::Plus)) span { "New" } }
    }
}

/// The filter box + toolbar actions + table + SSE-refreshed `<tbody>` --
/// everything below a page's own `<h1>`. Split out from `list_page` so a
/// page that needs its own custom header (the Services home page, with its
/// stats bar) can still reuse the table itself. `create` is the toolbar's
/// right-aligned create control(s) -- see `new_actions`.
pub fn list_table(
    spec: &ListSpec,
    units: &[(QuadletUnit, UnitStatus)],
    csrf: &str,
    rows_route: &str,
    all_units: &[QuadletUnit],
    groups: &GroupLists,
    create: Markup,
) -> Markup {
    html! {
        (known_groups_datalist(groups.known))
        div.toolbar {
            input.input.input-sm.filter-box type="search" data-filter-target=(ROWS_ID) placeholder="Filter…";
            div.toolbar-actions {
                (add_group_control(csrf))
                (create)
            }
        }
        div.table-wrap {
            // `data-synced-groups` -- the drag-and-drop guard in
            // `dragdrop.js` reads this once to refuse dropping a unit or
            // group into (or moving/renaming) a git-synced directory; the
            // server-side checks in `web::core` (`synced_destination` /
            // `synced_source`) are the authoritative gate either way, this
            // is just instant feedback instead of a rejected round trip.
            // Lives on `table-wrap`, outside the `tbody` htmx swaps, so it
            // survives a `units-changed` row refresh; it only goes stale if
            // a sync is added/removed while this page is already open --
            // reloading the page picks up the change.
            table.data-table data-synced-groups=(groups.synced.join(",")) {
                thead {
                    tr {
                        th { "File" }
                        @for column in spec.columns {
                            th { (column.header) }
                        }
                        th { "Status" }
                        th {}
                    }
                }
                // Only a create/edit/delete/move rebuilds the whole row set. A
                // status change updates each row's badge (its own `sse-swap`)
                // and the kebab's action forms (a scoped swap inside the
                // menu) in place -- swapping the whole `<tbody>` here would
                // slam shut any menu the user has open.
                tbody id=(ROWS_ID) hx-get=(rows_route) hx-trigger="sse:units-changed" hx-swap="innerHTML" {
                    (list_rows(spec, units, csrf, all_units, groups))
                }
            }
        }
    }
}

pub fn list_page(
    spec: &ListSpec,
    units: &[(QuadletUnit, UnitStatus)],
    csrf: &str,
    all_units: &[QuadletUnit],
    groups: &GroupLists,
    health: Health,
) -> Markup {
    let body = html! {
        (page_header(spec.title, html! {}))
        (list_table(
            spec, units, csrf, &rows_route(spec), all_units, groups, new_actions(spec),
        ))
    };
    shell(spec.title, spec.active_nav, Some(health), body)
}

/// The `/rows` fragment route lives at `{new_href's section}/rows` -- derived
/// from `new_href` (e.g. `/volumes/new` -> `/volumes/rows`) so a `ListSpec`
/// only needs to state its `new_href` once.
fn rows_route(spec: &ListSpec) -> String {
    let section = spec
        .new_href
        .rsplit_once('/')
        .map(|(prefix, _)| prefix)
        .unwrap_or(spec.new_href);
    format!("{section}/rows")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quadlet::UnitKind;
    use crate::systemd::UnitStatus;

    fn in_group(group: &str) -> (QuadletUnit, UnitStatus) {
        let unit = QuadletUnit {
            file_name: "x.container".into(),
            group: group.into(),
            path: "/tmp/x.container".into(),
            kind: UnitKind::Container,
            sections: Vec::new(),
            raw: String::new(),
        };
        (unit, UnitStatus::not_found())
    }

    #[test]
    fn nested_unit_count_includes_descendant_groups() {
        let units = [
            in_group(""),
            in_group("media"),
            in_group("media/arr"),
            in_group("media/arr/hd"),
            in_group("media-extra"),
        ];

        // Direct member + everything under `media/`, but not the sibling
        // `media-extra` (prefix match without a `/` boundary) or root units.
        assert_eq!(nested_unit_count(&units, "media"), 3);
        assert_eq!(nested_unit_count(&units, "media/arr"), 2);
        assert_eq!(nested_unit_count(&units, "media/arr/hd"), 1);
        assert_eq!(nested_unit_count(&units, "media-extra"), 1);
        assert_eq!(nested_unit_count(&units, "empty"), 0);
    }
}
