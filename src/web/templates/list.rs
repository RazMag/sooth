//! The generic, kind-parameterized list view shared by every section.
//! Columns differ per kind (see `templates::{volumes,networks,...}`); the row
//! shape, empty state, filter box, SSE refresh wiring, and the pod tree
//! (see [`PodTree`]) are identical everywhere, so they live here once.

use std::collections::HashMap;

use maud::{Markup, html};

use super::{
    Icon, NavItem, autostart_pill, autoupdate_pill, csrf_input, group_kebab, icon, kebab_menu,
    known_groups_datalist, page_header, shell, status_badge,
};
use crate::health::Health;
use crate::quadlet::refs::{self, MissingRefs};
use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;
use crate::web::core;

/// The DOM id of every list's `<tbody>` -- also the SSE-refresh target and
/// the `data-filter-target` of the filter box. One value everywhere.
pub const ROWS_ID: &str = "unit-rows";

/// The table-wide context every list render needs, bundled into one
/// parameter (keeps `list_table` under clippy's argument-count lint):
/// `known` is every existing group directory (drop target / "move to"
/// autocomplete source), `synced` is the subset of those git-sync already
/// manages -- see `web::core`'s `synced_destination` / `synced_source`,
/// the authoritative version of the same check this only previews for the
/// drag-and-drop guard in `dragdrop.js` and the "synced" chip below.
/// `missing` is each unit's unset secrets / host variables by file name
/// (`core::missing_refs`), drawn as a "missing" badge on its row.
pub struct ListContext<'a> {
    pub known: &'a [String],
    pub synced: &'a [String],
    pub missing: &'a HashMap<String, MissingRefs>,
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
    /// The kinds this table lists. Pods are loaded alongside whatever this
    /// says (see [`with_pods`]) so each one can head its units in the pod
    /// tree; a pod outside these kinds only renders when it has any.
    pub kinds: &'static [UnitKind],
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

/// Every kind in `kinds`, plus Pod -- what a list handler loads, so a pod
/// can head its units in any table (see [`ListSpec::kinds`]).
pub fn with_pods(kinds: &[UnitKind]) -> Vec<UnitKind> {
    let mut out = kinds.to_vec();
    if !out.contains(&UnitKind::Pod) {
        out.push(UnitKind::Pod);
    }
    out
}

/// A row's place in the pod tree.
#[derive(Clone, Copy)]
enum TreePos<'a> {
    /// No pod relation. `spacer` pads the name to line up with pod rows'
    /// toggles, in a table that has any.
    Plain { spacer: bool },
    /// A pod heading `children` rows of its own.
    Pod {
        children: &'a [&'a (QuadletUnit, UnitStatus)],
    },
    /// A unit shown under `pod` (see `refs::owning_pod`).
    Child { pod: &'a QuadletUnit },
}

/// Where a row renders: `group` is the section it sits in (`None` at the
/// root), which for a pod's child is its *pod's* group -- see [`PodTree`].
#[derive(Clone, Copy)]
struct Place<'a> {
    group: Option<&'a str>,
    tree: TreePos<'a>,
    /// Whether the unit's kind is one `ListSpec::kinds` lists.
    listed: bool,
}

fn row(
    (unit, status): &(QuadletUnit, UnitStatus),
    columns: &[Column],
    csrf: &str,
    all_units: &[QuadletUnit],
    lists: &ListContext,
    place: Place,
) -> Markup {
    let service = unit.service_name();
    let ctx = RowCtx {
        unit,
        status,
        all_units,
    };
    let move_url = format!("{}/move", core::unit_url(unit));
    // A member of `media/arr` renders one indent step past the "arr" header,
    // and a pod's child one step past its pod.
    let depth = place.group.map_or(0, |g| g.matches('/').count() + 1)
        + usize::from(matches!(place.tree, TreePos::Child { .. }));
    let (pod, pod_member) = match place.tree {
        TreePos::Pod { .. } => (Some(unit.file_name.as_str()), None),
        TreePos::Child { pod } => (None, Some(pod.file_name.as_str())),
        TreePos::Plain { .. } => (None, None),
    };
    html! {
        tr class=[place.group.map(|_| "is-collapsed")]
            data-group-member=[place.group]
            data-pod=[pod]
            data-pod-member=[pod_member]
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
                    @match place.tree {
                        TreePos::Pod { .. } => {
                            button.pod-toggle type="button" aria-expanded="true"
                                title="Show or hide this pod's units"
                                aria-label={"Toggle units of " (unit.file_name)} {
                                span.group-chevron aria-hidden="true" { (icon(Icon::ChevronDown)) }
                            }
                        }
                        TreePos::Plain { spacer: true } => { span.pod-toggle-spacer {} }
                        _ => {}
                    }
                    a href=(core::unit_url(unit)) { (unit.file_name) }
                    @if unit.is_template() {
                        " " span.chip.chip-muted title="Template unit — managed read-only, use the CLI to instantiate it" { "template" }
                    }
                    @if let TreePos::Child { pod } = place.tree {
                        @if unit.group != pod.group {
                            @let filed = if unit.group.is_empty() { "/" } else { unit.group.as_str() };
                            " " span.chip.chip-muted
                                title={"Filed under " (filed) " — shown here with its pod, " (pod.file_name)} {
                                (filed)
                            }
                        }
                    }
                    @if let Some(m) = lists.missing.get(&unit.file_name) {
                        " " (missing_badge(unit, m))
                    }
                    @match (unit.description(), place.tree) {
                        (desc, TreePos::Pod { children }) => {
                            div.cell-secondary {
                                @if let Some(desc) = desc { (desc) " · " }
                                (kind_counts(children))
                            }
                        }
                        (Some(desc), _) => { div.cell-secondary { (desc) } }
                        (None, _) => {}
                    }
                }
            }
            // A pod in a table that doesn't list pods is only here to head
            // its units -- that table's columns (Driver, Used by, ...) don't
            // describe it, so they stay blank rather than read "—".
            @for column in columns {
                td { @if place.listed { ((column.cell)(&ctx)) } }
            }
            td {
                (status_badge(&service, status))
                (autostart_pill(status.is_autostart_enabled()))
                (autoupdate_pill(unit))
            }
            td { (kebab_menu(unit, status, csrf, lists.known)) }
        }
    }
}

/// A pod's units summed up by kind, in the order they're listed under it --
/// "2 containers", or "2 containers, 1 volume, 1 network" on a table that
/// lists several kinds. Shown on the pod row's secondary line.
fn kind_counts(children: &[&(QuadletUnit, UnitStatus)]) -> String {
    let mut counts: Vec<(UnitKind, usize)> = Vec::new();
    for (unit, _) in children {
        match counts.iter_mut().find(|(k, _)| *k == unit.kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((unit.kind, 1)),
        }
    }
    counts
        .iter()
        .map(|(kind, n)| {
            let noun = kind.extension();
            if *n == 1 {
                format!("1 {noun}")
            } else {
                format!("{n} {noun}s")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A row's "N missing" badge, linking to the unit's detail page (which lists
/// each reference); the tooltip names them.
fn missing_badge(unit: &QuadletUnit, m: &MissingRefs) -> Markup {
    let mut parts = Vec::new();
    if !m.secrets.is_empty() {
        parts.push(format!("secrets: {}", m.secrets.join(", ")));
    }
    if !m.env.is_empty() {
        let vars: Vec<String> = m.env.iter().map(|n| format!("${{{n}}}")).collect();
        parts.push(format!("host variables: {}", vars.join(", ")));
    }
    let title = format!("Not set — {}", parts.join("; "));
    html! {
        a.badge.badge-warn href=(core::unit_url(unit)) title=(title) {
            (m.len()) " missing"
        }
    }
}

/// The list's units arranged as a pod tree: each unit `refs::owning_pod`
/// assigns to a pod that's also listed renders as that pod's child, directly
/// under the pod's row -- in the *pod's* group section, even when the child
/// is filed under another group (its row then carries a chip naming where).
/// Tree over directory, since a row can only render once: its status badge's
/// SSE id has to stay unique.
struct PodTree<'a> {
    /// Rows rendered at their section's top level, in load order: every
    /// listed unit without a listed pod, except a pod outside
    /// `ListSpec::kinds` that has no children here (it was only loaded to
    /// head them).
    tops: Vec<&'a (QuadletUnit, UnitStatus)>,
    /// Each pod's children, by the pod's file name.
    children: HashMap<&'a str, Vec<&'a (QuadletUnit, UnitStatus)>>,
    /// The group each listed unit renders in: its own, or its pod's.
    display_group: HashMap<&'a str, &'a str>,
}

impl<'a> PodTree<'a> {
    fn build(
        spec: &ListSpec,
        units: &'a [(QuadletUnit, UnitStatus)],
        all_units: &[QuadletUnit],
    ) -> Self {
        let listed_pod = |name: &str| {
            units
                .iter()
                .map(|(u, _)| u)
                .find(|u| u.kind == UnitKind::Pod && u.file_name == name)
        };
        let mut children: HashMap<&str, Vec<_>> = HashMap::new();
        let mut display_group = HashMap::new();
        let mut rest = Vec::new();
        for entry in units {
            let unit = &entry.0;
            match refs::owning_pod(unit, all_units).and_then(listed_pod) {
                Some(pod) => {
                    children
                        .entry(pod.file_name.as_str())
                        .or_default()
                        .push(entry);
                    display_group.insert(unit.file_name.as_str(), pod.group.as_str());
                }
                None => {
                    display_group.insert(unit.file_name.as_str(), unit.group.as_str());
                    rest.push(entry);
                }
            }
        }
        // Group a pod's units by kind (containers first), then by name.
        let rank = |k: UnitKind| UnitKind::all().iter().position(|x| *x == k);
        for kids in children.values_mut() {
            kids.sort_by(|(a, _), (b, _)| {
                (rank(a.kind), &a.file_name).cmp(&(rank(b.kind), &b.file_name))
            });
        }
        let tops = rest
            .into_iter()
            .filter(|(u, _)| {
                spec.kinds.contains(&u.kind) || children.contains_key(u.file_name.as_str())
            })
            .collect();
        PodTree {
            tops,
            children,
            display_group,
        }
    }

    fn group_of(&self, unit: &QuadletUnit) -> &'a str {
        self.display_group
            .get(unit.file_name.as_str())
            .copied()
            .unwrap_or_default()
    }
}

/// The full ordered set of group paths to render sections for: every group a
/// top-level row renders in, plus every group directory that exists on disk
/// (so a freshly-created but still-empty group shows up as a drop target).
/// Sorted, which puts each parent path immediately before its children.
fn section_groups<'a>(
    rendered: impl Iterator<Item = &'a str>,
    known_groups: &'a [String],
) -> Vec<&'a str> {
    let mut groups: Vec<&str> = rendered
        .filter(|g| !g.is_empty())
        .chain(known_groups.iter().map(String::as_str))
        .collect();
    groups.sort_unstable();
    groups.dedup();
    groups
}

/// How many of `groups` (one entry per listed unit) are `group` *or any of
/// its descendant groups*. This is the number a section header shows:
/// collapsing a group hides its subgroups and their rows too, so the count
/// has to speak for everything underneath, not just the units filed directly
/// in that directory.
fn nested_unit_count<'a>(groups: impl Iterator<Item = &'a str>, group: &str) -> usize {
    groups
        .filter(|g| {
            *g == group
                || g.strip_prefix(group)
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
                    // A synced group's own directory can be dragged to a new
                    // parent too -- see `web::core::move_group_dir`, which
                    // keeps `GitSyncConfig.group` pointed at wherever it
                    // ends up -- so it gets the same grip as any other group.
                    span.drag-handle.group-drag draggable="true"
                        title="Drag to move this group under another" aria-hidden="true" {
                        (icon(Icon::Grip))
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

/// A top-level row followed by its pod children, if it has any.
fn entry_rows(
    entry: &(QuadletUnit, UnitStatus),
    tree: &PodTree,
    has_pods: bool,
    spec: &ListSpec,
    csrf: &str,
    all_units: &[QuadletUnit],
    lists: &ListContext,
) -> Markup {
    let unit = &entry.0;
    let group = (!unit.group.is_empty()).then_some(unit.group.as_str());
    let kids = tree
        .children
        .get(unit.file_name.as_str())
        .map_or(&[][..], Vec::as_slice);
    let pos = if kids.is_empty() {
        TreePos::Plain { spacer: has_pods }
    } else {
        TreePos::Pod { children: kids }
    };
    let top = Place {
        group,
        tree: pos,
        listed: spec.kinds.contains(&unit.kind),
    };
    let child = Place {
        tree: TreePos::Child { pod: unit },
        listed: true,
        ..top
    };
    html! {
        (row(entry, spec.columns, csrf, all_units, lists, top))
        @for kid in kids {
            (row(kid, spec.columns, csrf, all_units, lists, child))
        }
    }
}

pub fn list_rows(
    spec: &ListSpec,
    units: &[(QuadletUnit, UnitStatus)],
    csrf: &str,
    all_units: &[QuadletUnit],
    groups: &ListContext,
) -> Markup {
    let tree = PodTree::build(spec, units, all_units);
    let sections = section_groups(
        tree.tops.iter().map(|(u, _)| u.group.as_str()),
        groups.known,
    );
    let colspan = spec.columns.len() + 3;
    if tree.tops.is_empty() && sections.is_empty() {
        return html! {
            tr { td colspan=(colspan) .empty { (spec.empty_hint) } }
        };
    }
    let has_pods = !tree.children.is_empty();
    // Section counts cover the kinds this table lists, not pods that are
    // only here to head them.
    let counted: Vec<&str> = units
        .iter()
        .filter(|(u, _)| spec.kinds.contains(&u.kind))
        .map(|(u, _)| tree.group_of(u))
        .collect();
    html! {
        // Root (ungrouped) units first, bare.
        @for entry in tree.tops.iter().filter(|(u, _)| u.group.is_empty()) {
            (entry_rows(entry, &tree, has_pods, spec, csrf, all_units, groups))
        }
        // Then one collapsible section per group directory.
        @for grp in sections {
            @let members: Vec<&&(QuadletUnit, UnitStatus)> =
                tree.tops.iter().filter(|(u, _)| u.group == grp).collect();
            @let nested = nested_unit_count(counted.iter().copied(), grp);
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
                @for entry in members {
                    (entry_rows(entry, &tree, has_pods, spec, csrf, all_units, groups))
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
    groups: &ListContext,
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
    groups: &ListContext,
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
    use crate::systemd::UnitStatus;

    use crate::quadlet::model::Section;

    fn unit(file_name: &str, kind: UnitKind, group: &str, entries: &[(&str, &str)]) -> QuadletUnit {
        QuadletUnit {
            file_name: file_name.into(),
            group: group.into(),
            path: format!("/tmp/{file_name}").into(),
            kind,
            sections: vec![Section {
                name: kind.primary_section().into(),
                entries: entries
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            }],
            raw: String::new(),
        }
    }

    fn spec(kinds: &'static [UnitKind]) -> ListSpec {
        ListSpec {
            title: "t",
            active_nav: None,
            kinds,
            columns: &[],
            new_href: "/t/new",
            empty_hint: "",
        }
    }

    fn names<'a>(rows: &[&'a (QuadletUnit, UnitStatus)]) -> Vec<&'a str> {
        rows.iter().map(|(u, _)| u.file_name.as_str()).collect()
    }

    #[test]
    fn pod_tree_nests_units_under_their_pod() {
        let all = vec![
            unit("web.pod", UnitKind::Pod, "web", &[]),
            unit("idle.pod", UnitKind::Pod, "", &[]),
            unit(
                "app.container",
                UnitKind::Container,
                "",
                &[("Pod", "web.pod"), ("Volume", "data.volume:/d")],
            ),
            unit("solo.container", UnitKind::Container, "", &[]),
            unit("data.volume", UnitKind::Volume, "", &[]),
            unit("loose.volume", UnitKind::Volume, "", &[]),
        ];
        let listed = |kinds: &[UnitKind]| -> Vec<(QuadletUnit, UnitStatus)> {
            all.iter()
                .filter(|u| with_pods(kinds).contains(&u.kind))
                .map(|u| (u.clone(), UnitStatus::not_found()))
                .collect()
        };

        // Services: every pod is listed in its own right; members nest.
        let units = listed(&[UnitKind::Container, UnitKind::Pod]);
        let services = spec(&[UnitKind::Container, UnitKind::Pod]);
        let tree = PodTree::build(&services, &units, &all);
        assert_eq!(names(&tree.tops), ["web.pod", "idle.pod", "solo.container"]);
        assert_eq!(names(&tree.children["web.pod"]), ["app.container"]);
        // A child renders in its pod's group, not its own.
        assert_eq!(tree.group_of(&all[2]), "web");

        // Volumes: a pod shows up only to head the volumes it owns.
        let units = listed(&[UnitKind::Volume]);
        let volumes = spec(&[UnitKind::Volume]);
        let tree = PodTree::build(&volumes, &units, &all);
        assert_eq!(names(&tree.tops), ["web.pod", "loose.volume"]);
        assert_eq!(names(&tree.children["web.pod"]), ["data.volume"]);
    }

    #[test]
    fn kind_counts_groups_by_kind_in_order() {
        let entry = |name: &str, kind| (unit(name, kind, "", &[]), UnitStatus::not_found());
        let rows = [
            entry("a.container", UnitKind::Container),
            entry("b.container", UnitKind::Container),
            entry("data.volume", UnitKind::Volume),
            entry("front.network", UnitKind::Network),
        ];
        let refs: Vec<_> = rows.iter().collect();
        assert_eq!(kind_counts(&refs), "2 containers, 1 volume, 1 network");
        assert_eq!(kind_counts(&refs[..1]), "1 container");
    }

    #[test]
    fn nested_unit_count_includes_descendant_groups() {
        let units = ["", "media", "media/arr", "media/arr/hd", "media-extra"];
        let nested_unit_count = |g| nested_unit_count(units.iter().copied(), g);

        // Direct member + everything under `media/`, but not the sibling
        // `media-extra` (prefix match without a `/` boundary) or root units.
        assert_eq!(nested_unit_count("media"), 3);
        assert_eq!(nested_unit_count("media/arr"), 2);
        assert_eq!(nested_unit_count("media/arr/hd"), 1);
        assert_eq!(nested_unit_count("media-extra"), 1);
        assert_eq!(nested_unit_count("empty"), 0);
    }
}
