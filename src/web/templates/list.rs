//! The generic, kind-parameterized list view shared by every section.
//! Columns differ per kind (see `templates::{volumes,networks,...}`); the row
//! shape, empty state, filter box, SSE refresh wiring, and the two tree
//! layouts (see [`ListView`]) are identical everywhere, so they live here once.

use std::collections::HashMap;

use maud::{Markup, html};

use super::{
    Icon, NavItem, autostart_icon, autoupdate_icon, csrf_input, group_kebab, icon, kebab_menu,
    kind_dot, known_groups_datalist, page_header, shell, status_badge,
};
use crate::health::Health;
use crate::quadlet::ports::SplitPorts;
use crate::quadlet::refs::{self, MissingRefs};
use crate::quadlet::{QuadletUnit, UnitKind, naming};
use crate::systemd::UnitStatus;
use crate::web::core;

/// The DOM id of every list's `<tbody>` -- also the SSE-refresh target and
/// the `data-filter-target` of the filter box. One value everywhere.
pub const ROWS_ID: &str = "unit-rows";

/// The `data-pod` key of the synthetic "Standalone" trunk in [`ListView::Pods`]
/// -- a `:` can't start a quadlet file name, so it never collides with a pod.
const STANDALONE: &str = ":standalone";

/// How a list table arranges its rows, picked per browser from the toolbar
/// (stored in the session, see `handlers::list::resolve_list_view`):
/// - `Pods`: a branch tree -- each pod heads the units it owns, a resource
///   several pods use shows as a linked leaf under each of them, and every
///   other unit hangs off a "Standalone" trunk. No group sections, no drag.
/// - `Dirs`: the group directories as collapsible sections, units flat
///   inside them (a pod member carries a chip naming its pod); drag-and-drop
///   filing and "Add group" live here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListView {
    #[default]
    Pods,
    Dirs,
}

impl ListView {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pods" => Some(Self::Pods),
            "dirs" => Some(Self::Dirs),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pods => "pods",
            Self::Dirs => "dirs",
        }
    }
}

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
    /// Pods' published ports split onto the members serving them, by file
    /// name (`core::split_pod_ports`) -- only for a table with a Ports
    /// column (Services); `None` elsewhere.
    pub ports: Option<&'a HashMap<String, SplitPorts>>,
    pub view: ListView,
}

/// What a column cell can draw on: the row's own unit and live status, plus
/// every quadlet on disk (all kinds) for columns that resolve cross-unit
/// references -- the "Used by" columns. `all_units` is an empty slice when
/// the caller didn't supply siblings.
pub struct RowCtx<'a> {
    pub unit: &'a QuadletUnit,
    pub status: &'a UnitStatus,
    pub all_units: &'a [QuadletUnit],
    /// This row's share of its pod's published ports, when they were split
    /// (see [`ListContext::ports`]).
    pub split_ports: Option<&'a SplitPorts>,
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

/// Every kind in `kinds`, plus Pod -- what a list handler loads, so a pod
/// can head its units in any table (see [`ListSpec::kinds`]).
pub fn with_pods(kinds: &[UnitKind]) -> Vec<UnitKind> {
    let mut out = kinds.to_vec();
    if !out.contains(&UnitKind::Pod) {
        out.push(UnitKind::Pod);
    }
    out
}

type Entry = (QuadletUnit, UnitStatus);

/// The branch connectors drawn at the start of a row's first cell, one
/// `--gstep`-wide column per tree level: `through[i]` is whether the
/// vertical line of level `i` (an ancestor's sibling list) continues past
/// this row, and `elbow` -- `Some(more)` for any nested row -- is the row's
/// own curved branch, `more` when further siblings follow it.
#[derive(Clone, Default)]
struct Rails {
    through: Vec<bool>,
    elbow: Option<bool>,
}

impl Rails {
    fn child(more: bool) -> Self {
        Rails {
            through: Vec::new(),
            elbow: Some(more),
        }
    }

    fn render(&self) -> Markup {
        html! {
            @if self.elbow.is_some() {
                span.rails aria-hidden="true" {
                    @for t in &self.through {
                        span.rail.rail-through[*t] {}
                    }
                    @if let Some(more) = self.elbow {
                        span.rail.rail-elbow.rail-more[more] {}
                    }
                }
            }
        }
    }
}

/// A row's role in the tree.
#[derive(Clone, Copy)]
enum TreePos<'a> {
    /// No pod relation drawn (every row in [`ListView::Dirs`]).
    Plain,
    /// A pod heading `children` (+ `leaves` shared resources) of its own.
    Pod {
        children: &'a [&'a Entry],
        leaves: usize,
    },
    /// A unit shown under `pod` (see `refs::owning_pod`), or under the
    /// Standalone trunk (`pod` is `None`).
    Child { pod: Option<&'a QuadletUnit> },
}

/// Where and how a row renders.
#[derive(Clone)]
struct Place<'a> {
    /// The group section the row sits in ([`ListView::Dirs`] only) -- what
    /// `groups.js` collapses it with and `dragdrop.js` files a drop under.
    group: Option<&'a str>,
    tree: TreePos<'a>,
    rails: Rails,
    /// Whether the unit's kind is one `ListSpec::kinds` lists.
    listed: bool,
}

fn row(
    (unit, status): &Entry,
    columns: &[Column],
    csrf: &str,
    all_units: &[QuadletUnit],
    lists: &ListContext,
    place: &Place,
) -> Markup {
    let service = unit.service_name();
    let ctx = RowCtx {
        unit,
        status,
        all_units,
        split_ports: lists.ports.and_then(|p| p.get(&unit.file_name)),
    };
    let dirs = lists.view == ListView::Dirs;
    let move_url = dirs.then(|| format!("{}/move", core::unit_url(unit)));
    let (pod, pod_member) = match place.tree {
        TreePos::Pod { .. } => (Some(unit.file_name.as_str()), None),
        TreePos::Child { pod } => (None, Some(pod.map_or(STANDALONE, |p| p.file_name.as_str()))),
        TreePos::Plain => (None, None),
    };
    let more = place.rails.elbow == Some(true);
    let classes: Vec<&str> = [
        pod.is_some().then_some("tree-trunk"),
        place.group.map(|_| "is-collapsed"),
    ]
    .into_iter()
    .flatten()
    .collect();
    html! {
        tr class=[(!classes.is_empty()).then(|| classes.join(" "))]
            data-group-member=[place.group]
            data-pod=[pod]
            data-pod-member=[pod_member]
            data-branch=[place.rails.elbow.map(|_| if more { "mid" } else { "last" })]
            data-move-url=[move_url] {
            td {
                div.tree-cell {
                    (place.rails.render())
                    @if dirs && !unit.is_template() {
                        span.drag-handle draggable="true"
                            title="Drag to file this unit under another group" aria-hidden="true" {
                            (icon(Icon::Grip))
                        }
                    }
                    @if let TreePos::Pod { children, leaves } = place.tree {
                        @if children.len() + leaves > 0 {
                            (pod_toggle(&unit.file_name))
                        }
                    }
                    (kind_dot(unit.kind))
                    div.tree-name {
                        div.tree-title {
                            a href=(core::unit_url(unit)) { (unit.file_name) }
                            @if unit.is_template() {
                                span.chip.chip-muted title="Template unit — managed read-only, use the CLI to instantiate it" { "template" }
                            }
                            (location_chips(unit, place, lists, all_units))
                            @if let Some(m) = lists.missing.get(&unit.file_name) {
                                (missing_badge(unit, m))
                            }
                        }
                        (secondary_line(unit, place, all_units))
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
                div.status-cell {
                    (status_badge(&service, status))
                    (autostart_icon(status.is_autostart_enabled()))
                    (autoupdate_icon(unit))
                }
            }
            td { (kebab_menu(unit, status, csrf, lists.known)) }
        }
    }
}

fn pod_toggle(pod: &str) -> Markup {
    html! {
        button.pod-toggle type="button" aria-expanded="true"
            title="Show or hide this pod's units"
            aria-label={"Toggle units of " (pod)} {
            span.group-chevron aria-hidden="true" { (icon(Icon::ChevronDown)) }
        }
    }
}

/// The chips after a row's name that say where it lives, when the layout
/// doesn't already: in [`ListView::Pods`], the directory a pod (or a unit
/// filed apart from its pod) sits in; in [`ListView::Dirs`], the pod a unit
/// belongs to.
fn location_chips(
    unit: &QuadletUnit,
    place: &Place,
    lists: &ListContext,
    all_units: &[QuadletUnit],
) -> Markup {
    match lists.view {
        ListView::Pods => {
            let shown = match place.tree {
                TreePos::Child { pod: Some(pod) } => unit.group != pod.group,
                _ => !unit.group.is_empty(),
            };
            let synced = lists
                .synced
                .iter()
                .any(|g| unit.group == *g || unit.group.starts_with(&format!("{g}/")));
            html! {
                @if shown {
                    @let filed = if unit.group.is_empty() { "/" } else { unit.group.as_str() };
                    span.chip.chip-muted.chip-folder title={"Filed under " (filed)} {
                        (icon(Icon::Folder)) (filed)
                    }
                }
                @if shown && synced {
                    a.chip.chip-synced href="/git-sync"
                        title="Synced from a git repository -- managed by the remote, see the Git Sync page" {
                        (icon(Icon::GitSync)) "synced"
                    }
                }
            }
        }
        ListView::Dirs => {
            let pod = refs::owning_pod(unit, all_units)
                .and_then(|name| all_units.iter().find(|u| u.file_name == name));
            html! {
                @if let Some(pod) = pod {
                    a.chip.chip-pod href=(core::unit_url(pod)) title={"Part of " (pod.file_name)} {
                        (icon(Icon::Pod)) (naming::stem(&pod.file_name))
                    }
                }
            }
        }
    }
}

/// A row's muted second line: a pod's member summary, a container's
/// resource chips, or the unit's `description()` for any other kind (a
/// container's is its image, which the Services table has a column for).
fn secondary_line(unit: &QuadletUnit, place: &Place, all_units: &[QuadletUnit]) -> Markup {
    if let TreePos::Pod { children, leaves } = place.tree {
        let containers: Vec<&UnitStatus> = children
            .iter()
            .filter(|(u, _)| u.kind == UnitKind::Container)
            .map(|(_, s)| s)
            .collect();
        return html! {
            div.cell-secondary {
                @if let Some(desc) = unit.description() { (desc) " · " }
                @if containers.is_empty() {
                    (kind_counts(children, leaves))
                } @else {
                    // `podtree.js` keeps this current as member badges
                    // change over SSE; the server value covers first paint.
                    span data-pod-summary {
                        (containers.iter().filter(|s| s.is_active()).count()) "/"
                        (containers.len()) " running"
                    }
                }
            }
        };
    }
    if unit.kind == UnitKind::Container {
        let resources = refs::container_resources(unit, all_units);
        return html! {
            @if !resources.is_empty() {
                div.cell-secondary.unit-refs {
                    @for name in &resources {
                        @if let Some(r) = all_units.iter().find(|u| &u.file_name == name) {
                            a.unit-ref href=(core::unit_url(r)) title=(name) {
                                (icon(kind_icon(r.kind))) (naming::stem(name))
                            }
                        }
                    }
                }
            }
        };
    }
    html! {
        @if let Some(desc) = unit.description() {
            div.cell-secondary { (desc) }
        }
    }
}

/// The icon a unit kind is drawn with -- in its `kind_dot` and its resource
/// chips.
pub fn kind_icon(kind: UnitKind) -> Icon {
    match kind {
        UnitKind::Container => Icon::Services,
        UnitKind::Pod => Icon::Pod,
        UnitKind::Volume => Icon::Volumes,
        UnitKind::Network => Icon::Networks,
        UnitKind::Image => Icon::Images,
        UnitKind::Build => Icon::Build,
        UnitKind::Kube => Icon::Kube,
    }
}

/// A pod's units summed up by kind, in the order they're listed under it --
/// "2 containers", or "2 volumes, 1 shared" on a table that also draws
/// shared leaves under it. Shown on the pod row's secondary line.
fn kind_counts(children: &[&Entry], leaves: usize) -> String {
    let mut counts: Vec<(UnitKind, usize)> = Vec::new();
    for (unit, _) in children {
        match counts.iter_mut().find(|(k, _)| *k == unit.kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((unit.kind, 1)),
        }
    }
    let mut parts: Vec<String> = counts
        .iter()
        .map(|(kind, n)| {
            let noun = kind.extension();
            if *n == 1 {
                format!("1 {noun}")
            } else {
                format!("{n} {noun}s")
            }
        })
        .collect();
    if leaves > 0 {
        parts.push(format!("{leaves} shared"));
    }
    parts.join(", ")
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

/// The list's units arranged as a pod tree ([`ListView::Pods`]): each unit
/// `refs::owning_pod` assigns to a listed pod renders as that pod's child,
/// directly under the pod's row. A resource no single pod owns but some
/// pods use renders once at the top level and again, as a link-only leaf
/// (no live badge -- its SSE id has to stay unique), under each of those
/// pods (`refs::consuming_pods`).
struct PodTree<'a> {
    /// The pods that head anything here, plus any pod the table lists in its
    /// own right, in load order.
    pods: Vec<&'a Entry>,
    /// Every listed unit without a listed pod, in load order -- the
    /// Standalone trunk's children.
    standalone: Vec<&'a Entry>,
    /// Each pod's children, by the pod's file name.
    children: HashMap<&'a str, Vec<&'a Entry>>,
    /// Each pod's shared-resource leaves, by the pod's file name.
    leaves: HashMap<&'a str, Vec<&'a Entry>>,
}

impl<'a> PodTree<'a> {
    fn build(spec: &ListSpec, units: &'a [Entry], all_units: &[QuadletUnit]) -> Self {
        let listed_pod = |name: &str| {
            units
                .iter()
                .map(|(u, _)| u)
                .find(|u| u.kind == UnitKind::Pod && u.file_name == name)
        };
        let mut children: HashMap<&str, Vec<_>> = HashMap::new();
        let mut leaves: HashMap<&str, Vec<_>> = HashMap::new();
        let mut rest = Vec::new();
        for entry in units {
            let unit = &entry.0;
            match refs::owning_pod(unit, all_units).and_then(listed_pod) {
                Some(pod) => children
                    .entry(pod.file_name.as_str())
                    .or_default()
                    .push(entry),
                None if unit.kind != UnitKind::Pod => {
                    if spec.kinds.contains(&unit.kind) {
                        for pod in refs::consuming_pods(unit, all_units)
                            .into_iter()
                            .filter_map(listed_pod)
                        {
                            leaves
                                .entry(pod.file_name.as_str())
                                .or_default()
                                .push(entry);
                        }
                    }
                    rest.push(entry);
                }
                None => rest.push(entry),
            }
        }
        // Group a pod's units by kind (containers first), then by name.
        let rank = |k: UnitKind| UnitKind::all().iter().position(|x| *x == k);
        for kids in children.values_mut().chain(leaves.values_mut()) {
            kids.sort_by(|(a, _), (b, _)| {
                (rank(a.kind), &a.file_name).cmp(&(rank(b.kind), &b.file_name))
            });
        }
        let (pods, standalone) = rest
            .into_iter()
            .filter(|(u, _)| {
                spec.kinds.contains(&u.kind)
                    || children.contains_key(u.file_name.as_str())
                    || leaves.contains_key(u.file_name.as_str())
            })
            .partition(|(u, _)| u.kind == UnitKind::Pod);
        PodTree {
            pods,
            standalone,
            children,
            leaves,
        }
    }

    fn children_of(&self, pod: &QuadletUnit) -> &[&'a Entry] {
        self.children
            .get(pod.file_name.as_str())
            .map_or(&[][..], Vec::as_slice)
    }

    fn leaves_of(&self, pod: &QuadletUnit) -> &[&'a Entry] {
        self.leaves
            .get(pod.file_name.as_str())
            .map_or(&[][..], Vec::as_slice)
    }
}

/// A link-only row standing in for a shared resource under one of the pods
/// that use it -- the real row (live badge, menu) is under Standalone.
fn leaf_row(
    (unit, _): &Entry,
    pod: &QuadletUnit,
    more: bool,
    colspan: usize,
    all: &[QuadletUnit],
) -> Markup {
    let consumers = refs::consumers_of(unit, all);
    html! {
        tr.tree-leaf data-pod-member=(pod.file_name) data-branch=(if more { "mid" } else { "last" }) {
            td colspan=(colspan - 1) {
                div.tree-cell {
                    (Rails::child(more).render())
                    (kind_dot(unit.kind))
                    div.tree-name {
                        div.tree-title {
                            a href=(core::unit_url(unit)) { (unit.file_name) }
                            span.chip.chip-muted title={"Also used by: " (consumers.join(", "))} {
                                (icon(Icon::Link)) "shared · " (consumers.len()) " consumers"
                            }
                        }
                    }
                }
            }
            td {}
        }
    }
}

fn pod_rows(
    spec: &ListSpec,
    units: &[Entry],
    csrf: &str,
    all_units: &[QuadletUnit],
    lists: &ListContext,
) -> Markup {
    let tree = PodTree::build(spec, units, all_units);
    let colspan = spec.columns.len() + 3;
    if tree.pods.is_empty() && tree.standalone.is_empty() {
        return html! { tr { td colspan=(colspan) .empty { (spec.empty_hint) } } };
    }
    // Without any pod trunk a "Standalone" heading would be the whole table
    // -- the units just render bare.
    let trunked = !tree.pods.is_empty();
    html! {
        @for pod in &tree.pods {
            @let kids = tree.children_of(&pod.0);
            @let leaves = tree.leaves_of(&pod.0);
            (row(pod, spec.columns, csrf, all_units, lists, &Place {
                group: None,
                tree: TreePos::Pod { children: kids, leaves: leaves.len() },
                rails: Rails::default(),
                listed: spec.kinds.contains(&pod.0.kind),
            }))
            @for (i, kid) in kids.iter().enumerate() {
                (row(kid, spec.columns, csrf, all_units, lists, &Place {
                    group: None,
                    tree: TreePos::Child { pod: Some(&pod.0) },
                    rails: Rails::child(i + 1 < kids.len() + leaves.len()),
                    listed: true,
                }))
            }
            @for (i, leaf) in leaves.iter().enumerate() {
                (leaf_row(leaf, &pod.0, i + 1 < leaves.len(), colspan, all_units))
            }
        }
        @if trunked && !tree.standalone.is_empty() {
            (standalone_row(tree.standalone.len(), colspan))
        }
        @for (i, entry) in tree.standalone.iter().enumerate() {
            (row(entry, spec.columns, csrf, all_units, lists, &Place {
                group: None,
                tree: if trunked { TreePos::Child { pod: None } } else { TreePos::Plain },
                rails: if trunked {
                    Rails::child(i + 1 < tree.standalone.len())
                } else {
                    Rails::default()
                },
                listed: true,
            }))
        }
    }
}

/// The Standalone trunk: heads every unit no single listed pod owns.
fn standalone_row(count: usize, colspan: usize) -> Markup {
    html! {
        tr.tree-trunk.standalone-row data-pod=(STANDALONE) {
            td colspan=(colspan) {
                div.tree-cell {
                    (pod_toggle("standalone units"))
                    span.kind-dot.kind-standalone aria-hidden="true" { (icon(Icon::Standalone)) }
                    div.tree-name {
                        div.tree-title { span.tree-label { "Standalone" } }
                        div.cell-secondary { (count) " not owned by a single pod" }
                    }
                }
            }
        }
    }
}

/// The full ordered set of group paths to render sections for: every group a
/// row renders in, plus every group directory that exists on disk (so a
/// freshly-created but still-empty group shows up as a drop target). Sorted,
/// which puts each parent path immediately before its children.
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

/// The directory tree's shape, for drawing [`Rails`] in [`ListView::Dirs`]:
/// which sections have subgroups, and which are followed by a sibling.
struct DirShape<'a> {
    sections: &'a [&'a str],
}

impl DirShape<'_> {
    fn parent(path: &str) -> Option<&str> {
        path.rsplit_once('/').map(|(p, _)| p)
    }

    fn has_subgroups(&self, group: &str) -> bool {
        self.sections.iter().any(|g| Self::parent(g) == Some(group))
    }

    /// Whether a later sibling section follows `group` under its parent.
    fn has_next_sibling(&self, group: &str) -> bool {
        let parent = Self::parent(group);
        self.sections
            .iter()
            .any(|g| Self::parent(g) == parent && *g > group)
    }

    /// The pass-through lines for a row nested under `group`'s chain of
    /// ancestors, from the top-level section's children down: one per
    /// ancestor below the top level, each continuing while that ancestor
    /// still has a later sibling.
    fn through(&self, group: &str) -> Vec<bool> {
        let segs: Vec<&str> = group.split('/').collect();
        (2..=segs.len())
            .map(|n| self.has_next_sibling(&segs[..n].join("/")))
            .collect()
    }

    /// Rails for `group`'s own header row (no branch at the top level).
    fn header(&self, group: &str) -> Rails {
        match Self::parent(group) {
            None => Rails::default(),
            Some(parent) => Rails {
                through: self.through(parent),
                elbow: Some(self.has_next_sibling(group)),
            },
        }
    }

    /// Rails for a row directly inside `group`; `more` when another member
    /// row follows it.
    fn member(&self, group: &str, more: bool) -> Rails {
        Rails {
            through: self.through(group),
            elbow: Some(more || self.has_subgroups(group)),
        }
    }
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
fn group_header_row(
    path: &str,
    count: usize,
    colspan: usize,
    csrf: &str,
    synced: bool,
    rails: &Rails,
) -> Markup {
    let last = path.rsplit('/').next().unwrap_or(path);
    html! {
        tr.group-row data-group=(path)
            data-branch=[rails.elbow.map(|more| if more { "mid" } else { "last" })] {
            td.group-head-cell colspan=(colspan - 1) {
                div.tree-cell {
                    (rails.render())
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
                        span.kind-dot.kind-folder aria-hidden="true" { (icon(Icon::Folder)) }
                        span.group-name { (last) "/" }
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

fn dir_rows(
    spec: &ListSpec,
    units: &[Entry],
    csrf: &str,
    all_units: &[QuadletUnit],
    lists: &ListContext,
) -> Markup {
    // Only the kinds this table lists -- a pod outside them was loaded to
    // head its units in the pod tree, and there's no tree here.
    let listed: Vec<&Entry> = units
        .iter()
        .filter(|(u, _)| spec.kinds.contains(&u.kind))
        .collect();
    let sections = section_groups(listed.iter().map(|(u, _)| u.group.as_str()), lists.known);
    let colspan = spec.columns.len() + 3;
    if listed.is_empty() && sections.is_empty() {
        return html! { tr { td colspan=(colspan) .empty { (spec.empty_hint) } } };
    }
    let shape = DirShape {
        sections: &sections,
    };
    html! {
        // Root (ungrouped) units first, bare.
        @for entry in listed.iter().filter(|(u, _)| u.group.is_empty()) {
            (row(entry, spec.columns, csrf, all_units, lists, &Place {
                group: None,
                tree: TreePos::Plain,
                rails: Rails::default(),
                listed: true,
            }))
        }
        // Then one collapsible section per group directory.
        @for grp in &sections {
            @let members: Vec<&&Entry> = listed.iter().filter(|(u, _)| u.group == *grp).collect();
            @let nested = nested_unit_count(listed.iter().map(|(u, _)| u.group.as_str()), grp);
            @let synced = lists.synced.iter().any(|g| g == grp);
            (group_header_row(grp, nested, colspan, csrf, synced, &shape.header(grp)))
            @if members.is_empty() && nested == 0 {
                tr.group-empty.is-collapsed data-group-member=(grp) {
                    td colspan=(colspan) {
                        div.tree-cell {
                            (shape.member(grp, false).render())
                            span.muted { "Empty — drag a unit here to file it under this group." }
                        }
                    }
                }
            } @else {
                @for (i, entry) in members.iter().enumerate() {
                    (row(entry, spec.columns, csrf, all_units, lists, &Place {
                        group: Some(grp),
                        tree: TreePos::Plain,
                        rails: shape.member(grp, i + 1 < members.len()),
                        listed: true,
                    }))
                }
            }
        }
    }
}

pub fn list_rows(
    spec: &ListSpec,
    units: &[Entry],
    csrf: &str,
    all_units: &[QuadletUnit],
    lists: &ListContext,
) -> Markup {
    match lists.view {
        ListView::Pods => pod_rows(spec, units, csrf, all_units, lists),
        ListView::Dirs => dir_rows(spec, units, csrf, all_units, lists),
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

/// The toolbar's Pods / Directories switch: plain links (works without JS)
/// back to the same page with `?view=`, which the handler remembers in the
/// session for this page and every later `/rows` refresh.
fn view_toggle(current: ListView) -> Markup {
    let opt = |view: ListView, ic: Icon, label: &str| {
        html! {
            a.view-opt.is-active[current == view] href={"?view=" (view.as_str())}
                aria-current=[(current == view).then_some("true")] {
                (icon(ic)) span { (label) }
            }
        }
    };
    html! {
        nav.view-toggle aria-label="Table layout" {
            (opt(ListView::Pods, Icon::Pod, "Pods"))
            (opt(ListView::Dirs, Icon::Folder, "Directories"))
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
    units: &[Entry],
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
            (view_toggle(groups.view))
            div.toolbar-actions {
                @if groups.view == ListView::Dirs {
                    (add_group_control(csrf))
                }
                (create)
            }
        }
        div.table-wrap.tree-wrap {
            // `data-synced-groups` -- the drag-and-drop guard in
            // `dragdrop.js` reads this once to refuse dropping a unit or
            // group into (or moving/renaming) a git-synced directory; the
            // server-side checks in `web::core` (`synced_destination` /
            // `synced_source`) are the authoritative gate either way, this
            // is just instant feedback instead of a rejected round trip.
            // Lives on `table-wrap`, outside the `tbody` htmx swaps, so it
            // survives a `units-changed` row refresh; it only goes stale if
            // a sync is added/removed while this page is already open --
            // reloading the page picks up the change. `data-view` gates
            // drag-and-drop to the Directories layout.
            table.data-table.tree-table data-view=(groups.view.as_str())
                data-synced-groups=(groups.synced.join(",")) {
                thead {
                    tr {
                        th { "Name" }
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
    units: &[Entry],
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

    fn names<'a>(rows: &[&'a Entry]) -> Vec<&'a str> {
        rows.iter().map(|(u, _)| u.file_name.as_str()).collect()
    }

    fn fixture() -> Vec<QuadletUnit> {
        vec![
            unit(
                "web.pod",
                UnitKind::Pod,
                "web",
                &[("Network", "proxy.network")],
            ),
            unit("idle.pod", UnitKind::Pod, "", &[]),
            unit("db.pod", UnitKind::Pod, "", &[]),
            unit(
                "app.container",
                UnitKind::Container,
                "",
                &[("Pod", "web.pod"), ("Volume", "data.volume:/d")],
            ),
            unit(
                "pg.container",
                UnitKind::Container,
                "",
                &[("Pod", "db.pod"), ("Network", "proxy.network")],
            ),
            unit("solo.container", UnitKind::Container, "", &[]),
            unit("data.volume", UnitKind::Volume, "", &[]),
            unit("loose.volume", UnitKind::Volume, "", &[]),
            unit("proxy.network", UnitKind::Network, "infra", &[]),
        ]
    }

    fn listed(all: &[QuadletUnit], kinds: &[UnitKind]) -> Vec<Entry> {
        all.iter()
            .filter(|u| with_pods(kinds).contains(&u.kind))
            .map(|u| (u.clone(), UnitStatus::not_found()))
            .collect()
    }

    fn render(spec: &ListSpec, units: &[Entry], all: &[QuadletUnit], view: ListView) -> String {
        let missing = HashMap::new();
        let lists = ListContext {
            known: &[],
            synced: &[],
            missing: &missing,
            ports: None,
            view,
        };
        list_rows(spec, units, "csrf", all, &lists).into_string()
    }

    #[test]
    fn pod_tree_nests_units_under_their_pod() {
        let all = fixture();

        // Services: every pod is listed in its own right; members nest.
        let units = listed(&all, &[UnitKind::Container, UnitKind::Pod]);
        let services = spec(&[UnitKind::Container, UnitKind::Pod]);
        let tree = PodTree::build(&services, &units, &all);
        assert_eq!(names(&tree.pods), ["web.pod", "idle.pod", "db.pod"]);
        assert_eq!(names(&tree.standalone), ["solo.container"]);
        assert_eq!(names(&tree.children["web.pod"]), ["app.container"]);
        assert!(tree.leaves.is_empty());

        // Volumes: a pod shows up only to head the volumes it owns.
        let units = listed(&all, &[UnitKind::Volume]);
        let volumes = spec(&[UnitKind::Volume]);
        let tree = PodTree::build(&volumes, &units, &all);
        assert_eq!(names(&tree.pods), ["web.pod"]);
        assert_eq!(names(&tree.standalone), ["loose.volume"]);
        assert_eq!(names(&tree.children["web.pod"]), ["data.volume"]);
    }

    #[test]
    fn shared_resource_is_a_leaf_under_each_pod_and_one_real_row() {
        let all = fixture();
        let units = listed(&all, &[UnitKind::Network]);
        let networks = spec(&[UnitKind::Network]);
        let tree = PodTree::build(&networks, &units, &all);
        // Neither pod owns the network, but both use it.
        assert_eq!(names(&tree.pods), ["web.pod", "db.pod"]);
        assert_eq!(names(&tree.standalone), ["proxy.network"]);
        assert_eq!(names(&tree.leaves["web.pod"]), ["proxy.network"]);
        assert_eq!(names(&tree.leaves["db.pod"]), ["proxy.network"]);

        let html = render(&networks, &units, &all, ListView::Pods);
        // One live badge (its SSE id must stay unique), two link-only leaves.
        assert_eq!(
            html.matches("sse-swap=\"status-proxy-network.service\"")
                .count(),
            1
        );
        assert_eq!(html.matches("class=\"tree-leaf\"").count(), 2);
        assert!(html.contains("data-pod=\":standalone\""));
        // No drag-and-drop in the pod layout.
        assert!(!html.contains("data-move-url"));
    }

    #[test]
    fn pods_layout_without_pods_renders_bare_rows() {
        let all = vec![unit("solo.container", UnitKind::Container, "", &[])];
        let units = listed(&all, &[UnitKind::Container]);
        let html = render(&spec(&[UnitKind::Container]), &units, &all, ListView::Pods);
        assert!(!html.contains(":standalone"));
        assert!(html.contains("solo.container"));
    }

    #[test]
    fn dirs_layout_is_flat_with_pod_chips() {
        let all = fixture();
        let units = listed(&all, &[UnitKind::Container]);
        let html = render(&spec(&[UnitKind::Container]), &units, &all, ListView::Dirs);
        // No pod nesting, and pods outside the listed kinds aren't drawn.
        assert!(!html.contains("data-pod="));
        assert!(!html.contains("data-pod-member="));
        assert!(!html.contains(">web.pod<"));
        // A member names its pod instead.
        assert!(html.contains("class=\"chip chip-pod\""));
        assert!(html.contains("data-move-url=\"/containers/app.container/move\""));
    }

    #[test]
    fn dir_shape_draws_through_lines_for_later_siblings() {
        let sections = ["media", "media/arr", "media/arr/hd", "media/tv", "zeta"];
        let shape = DirShape {
            sections: &sections,
        };
        // Top-level sections get no branch.
        assert_eq!(shape.header("media").elbow, None);
        // `media/arr` is followed by `media/tv`.
        assert_eq!(shape.header("media/arr").elbow, Some(true));
        assert_eq!(shape.header("media/tv").elbow, Some(false));
        // A row inside `media/arr/hd` continues `media/arr`'s line.
        assert_eq!(shape.member("media/arr/hd", false).through, [true, false]);
        // A member of `media` branches on: subgroups follow it.
        assert_eq!(shape.member("media", false).elbow, Some(true));
        assert_eq!(shape.member("zeta", false).elbow, Some(false));
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
        assert_eq!(kind_counts(&refs, 0), "2 containers, 1 volume, 1 network");
        assert_eq!(kind_counts(&refs[..1], 0), "1 container");
        assert_eq!(kind_counts(&refs[2..3], 2), "1 volume, 2 shared");
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
