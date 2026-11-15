//! The Overview home page: one sentence on how things are, four tiles, this
//! machine's card, and "Needs you" -- the problems worth acting on, each
//! with the action that fixes it. In Phase 1 there is one machine; the hub
//! will list every machine here once the fleet layer exists.

use maud::{Markup, html};

use super::services::Stats;
use super::{Icon, NavItem, icon, rough_span, shell};
use crate::activity::{Entry, Kind};
use crate::health::Health;

/// What the host says about itself, for the machine card.
pub struct MachineInfo {
    pub hostname: String,
    pub arch: &'static str,
    /// `/etc/os-release`'s `PRETTY_NAME`, when readable.
    pub os: Option<String>,
}

impl MachineInfo {
    pub fn detect() -> Self {
        let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "this machine".to_string());
        let os = std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|s| pretty_name(&s));
        Self {
            hostname,
            arch: std::env::consts::ARCH,
            os,
        }
    }

    /// Two letters for the machine's avatar.
    fn initials(&self) -> String {
        self.hostname
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(2)
            .collect::<String>()
            .to_uppercase()
    }
}

fn pretty_name(os_release: &str) -> Option<String> {
    os_release.lines().find_map(|line| {
        let value = line.strip_prefix("PRETTY_NAME=")?;
        let value = value.trim().trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// A failed unit, with where to look at it.
pub struct FailedUnit {
    pub name: String,
    pub href: String,
    /// "exit 1 · 2h ago", as the status cell shows it.
    pub detail: String,
}

/// A secret some units reference that isn't set.
pub struct MissingSecret {
    pub name: String,
    pub units: usize,
}

/// The Git Sync tile's numbers.
#[derive(Default)]
pub struct SyncSummary {
    pub repos: usize,
    /// Repos whose last sync failed or diverged.
    pub attention: usize,
}

pub struct Overview {
    pub machine: MachineInfo,
    pub stats: Stats,
    pub failed: Vec<FailedUnit>,
    pub missing_secrets: Vec<MissingSecret>,
    /// Containers pinned to an image with newer tags.
    pub newer_tags: Vec<String>,
    pub sync: SyncSummary,
    pub health: Health,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "35 of 41 services are up", or the all-up / empty wording.
fn headline(stats: &Stats) -> String {
    match (stats.total, stats.running) {
        (0, _) => "No services yet".to_string(),
        (1, 1) => "Your one service is up".to_string(),
        (t, r) if r == t => format!("All {t} services are up"),
        (t, r) => format!("{r} of {t} services are up"),
    }
}

/// The sentence under the headline: each problem in a few words, or a
/// reassurance when there are none.
fn summary(o: &Overview) -> String {
    let mut parts: Vec<String> = Vec::new();
    match o.failed.as_slice() {
        [] => {}
        [one] => parts.push(format!("{} failed", one.name)),
        many => parts.push(format!("{} failed", plural(many.len(), "unit", "units"))),
    }
    match o.missing_secrets.len() {
        0 => {}
        1 => parts.push(format!("secret {} isn't set", o.missing_secrets[0].name)),
        n => parts.push(format!("{n} secrets aren't set")),
    }
    if o.health.linger_enabled == Some(false) {
        parts.push("linger is off".to_string());
    }
    if o.sync.attention > 0 {
        parts.push(format!(
            "{} attention",
            plural(
                o.sync.attention,
                "Git Sync repo needs",
                "Git Sync repos need"
            )
        ));
    }
    let Some(last) = parts.pop() else {
        return format!("Nothing needs you on {}.", o.machine.hostname);
    };
    let mut sentence = if parts.is_empty() {
        last
    } else {
        format!("{} and {last}", parts.join(", "))
    };
    // Sentence case, without touching a unit name that starts it.
    if o.failed.len() != 1
        && let Some(first) = sentence.get(..1)
    {
        sentence = first.to_uppercase() + &sentence[1..];
    }
    format!("{sentence}.")
}

fn tile(label: &str, value: String, note: Markup, href: &str, alert: bool) -> Markup {
    html! {
        a class=(if alert { "stat-card stat-card-alert" } else { "stat-card" }) href=(href) {
            div.stat-card-label { (label) }
            div.stat-card-value { (value) }
            div.stat-card-note { (note) }
        }
    }
}

fn tiles(o: &Overview) -> Markup {
    let services = NavItem::Services.href();
    html! {
        div.stat-grid.overview-tiles {
            (tile("Running", o.stats.running.to_string(),
                html! { "of " (o.stats.total) }, services, false))
            (tile("Failed", o.stats.failed.to_string(), html! {
                @match o.failed.as_slice() {
                    [] => "none",
                    [one] => (one.name),
                    many => (plural(many.len(), "unit", "units")),
                }
            }, services, o.stats.failed > 0))
            (tile("Newer image tags", o.newer_tags.len().to_string(),
                html! { (if o.newer_tags.len() == 1 { "pinned image" } else { "pinned images" }) },
                services, false))
            (tile("Git Sync", o.sync.repos.to_string(), html! {
                (if o.sync.repos == 1 { "repo" } else { "repos" })
                @if o.sync.attention > 0 { " · " (o.sync.attention) " " (if o.sync.attention == 1 { "needs" } else { "need" }) " attention" }
            }, NavItem::GitSync.href(), o.sync.attention > 0))
        }
    }
}

fn machine_card(o: &Overview) -> Markup {
    let m = &o.machine;
    let stopped = o.stats.total - o.stats.running - o.stats.failed;
    html! {
        section.card.machine-card {
            div.machine-head {
                span.machine-avatar aria-hidden="true" { (m.initials()) }
                div {
                    div.machine-name { (m.hostname) }
                    div.machine-meta {
                        (m.arch)
                        @if let Some(os) = &m.os { " · " (os) }
                    }
                }
            }
            div.machine-counts {
                span.count-ok { (o.stats.running) " up" }
                @if o.stats.failed > 0 { " · " span.count-failed { (o.stats.failed) " failed" } }
                @if stopped > 0 { " · " (stopped) " off" }
            }
            @if o.health.linger_enabled == Some(false) {
                div.machine-warn {
                    "Linger is off: units stop at logout. "
                    a href=(NavItem::Settings.href()) { "Fix" }
                }
            }
        }
    }
}

fn need(title: Markup, note: Markup, actions: Markup) -> Markup {
    html! {
        li.need {
            div.need-text {
                div.need-title { (title) }
                div.need-note { (note) }
            }
            div.need-actions { (actions) }
        }
    }
}

fn needs_you(o: &Overview) -> Markup {
    let any = !o.failed.is_empty()
        || !o.missing_secrets.is_empty()
        || !o.newer_tags.is_empty()
        || o.sync.attention > 0
        || !o.health.podman_generator_found;
    html! {
        section.card.needs-you {
            h2.card-title { "Needs you" }
            @if !any {
                p.muted { "Nothing right now." }
            } @else {
                ul.need-list {
                    @for f in &o.failed {
                        (need(
                            html! { strong { (f.name) } " failed" },
                            html! { (f.detail) },
                            html! {
                                a.btn.btn-sm href={(f.href) "/logs"} { (icon(Icon::Logs)) span { "Logs" } }
                                a.btn.btn-sm href=(f.href) { "Open" }
                            },
                        ))
                    }
                    @for s in &o.missing_secrets {
                        (need(
                            html! { "Secret " code { (s.name) } " not set" },
                            html! { "Referenced by " (plural(s.units, "unit", "units")) },
                            html! { a.btn.btn-sm href=(NavItem::Secrets.href()) { "Set secret" } },
                        ))
                    }
                    @if !o.newer_tags.is_empty() {
                        (need(
                            html! { (plural(o.newer_tags.len(), "pinned image has", "pinned images have")) " newer tags" },
                            html! { (o.newer_tags.join(", ")) },
                            html! { a.btn.btn-sm href=(NavItem::Services.href()) { "Review tags" } },
                        ))
                    }
                    @if o.sync.attention > 0 {
                        (need(
                            html! { (plural(o.sync.attention, "Git Sync repo needs", "Git Sync repos need")) " attention" },
                            html! { "The last sync failed or the checkout diverged" },
                            html! { a.btn.btn-sm href=(NavItem::GitSync.href()) { "Open Git Sync" } },
                        ))
                    }
                    @if !o.health.podman_generator_found {
                        (need(
                            html! { "Podman's quadlet generator wasn't found" },
                            html! { "Units won't start until Podman is installed" },
                            html! { a.btn.btn-sm href=(NavItem::Settings.href()) { "Open Settings" } },
                        ))
                    }
                }
            }
        }
    }
}

/// Everything below the page header -- also the `/overview/body` fragment
/// the page re-fetches when a unit or its status changes.
pub fn overview_body(o: &Overview) -> Markup {
    html! {
        p.overview-summary { (summary(o)) }
        (tiles(o))
        div.overview-grid {
            (machine_card(o))
            (needs_you(o))
        }
    }
}

/// The Activity feed's entries, newest first -- also the
/// `/overview/activity` fragment, re-fetched when an entry is added. `now`
/// is Unix seconds.
pub fn activity_items(entries: &[Entry], now: u64) -> Markup {
    html! {
        @if entries.is_empty() {
            li.act-empty.muted { "Nothing yet. Units starting, stopping or failing show up here." }
        }
        @for e in entries {
            li class=(match e.kind { Kind::Problem => "act-item act-problem", Kind::Change => "act-item act-change" }) {
                span.act-dot aria-hidden="true" {}
                span.act-text { strong { (e.subject) } " " (e.text) }
                span.act-time title=(e.at) {
                    @if e.at <= now { (rough_span((now - e.at) * 1_000_000)) " ago" }
                }
            }
        }
    }
}

/// The Activity card: an All / Problems / Changes filter (pure CSS, see
/// `.activity` in `styles.css`) above the live list.
fn activity_card(entries: &[Entry], now: u64) -> Markup {
    const FILTERS: [(&str, &str); 3] = [
        ("all", "All"),
        ("problem", "Problems"),
        ("change", "Changes"),
    ];
    html! {
        section.card.activity {
            div.activity-head {
                h2.card-title { "Activity" }
                div.segmented.segmented-sm role="radiogroup" aria-label="Show" {
                    @for (value, label) in FILTERS {
                        @let id = format!("activity-{value}");
                        input type="radio" name="activity-filter" id=(id) value=(value) checked[value == "all"];
                        label for=(id) { (label) }
                    }
                }
            }
            ul.act-list id="overview-activity" hx-get="/overview/activity"
                hx-trigger="sse:activity-changed" hx-swap="innerHTML" {
                (activity_items(entries, now))
            }
        }
    }
}

pub fn overview_page(o: &Overview, activity: &[Entry], now: u64) -> Markup {
    let body = html! {
        div.page-header.overview-header {
            h1 id="overview-headline" { (headline(&o.stats)) }
            div.actions {
                a.btn.btn-sm.btn-primary href="/containers/new" { (icon(Icon::Plus)) span { "New unit" } }
            }
        }
        div id="overview-body" hx-get="/overview/body"
            hx-trigger="sse:units-changed, sse:any-status delay:500ms, sse:image-tags-changed"
            hx-swap="innerHTML" {
            (overview_body(o))
        }
        (activity_card(activity, now))
    };
    shell(
        &headline(&o.stats),
        Some(NavItem::Overview),
        Some(o.health),
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overview() -> Overview {
        Overview {
            machine: MachineInfo {
                hostname: "nas01".into(),
                arch: "x86_64",
                os: Some("Fedora Linux 43".into()),
            },
            stats: Stats {
                total: 14,
                running: 12,
                failed: 1,
            },
            failed: vec![FailedUnit {
                name: "radarr.container".into(),
                href: "/containers/radarr.container".into(),
                detail: "exit 1 · 2h ago".into(),
            }],
            missing_secrets: vec![],
            newer_tags: vec![],
            sync: SyncSummary::default(),
            health: Health {
                podman_generator_found: true,
                linger_enabled: Some(true),
            },
        }
    }

    #[test]
    fn headline_counts_services_up() {
        let s = |total, running| Stats {
            total,
            running,
            failed: 0,
        };
        assert_eq!(headline(&s(0, 0)), "No services yet");
        assert_eq!(headline(&s(1, 1)), "Your one service is up");
        assert_eq!(headline(&s(4, 4)), "All 4 services are up");
        assert_eq!(headline(&s(41, 35)), "35 of 41 services are up");
    }

    #[test]
    fn summary_names_each_problem() {
        let mut o = overview();
        assert_eq!(summary(&o), "radarr.container failed.");
        o.missing_secrets.push(MissingSecret {
            name: "db-pass".into(),
            units: 2,
        });
        o.health.linger_enabled = Some(false);
        assert_eq!(
            summary(&o),
            "radarr.container failed, secret db-pass isn't set and linger is off."
        );
        o.failed.clear();
        o.missing_secrets.clear();
        assert_eq!(summary(&o), "Linger is off.");
        o.health.linger_enabled = Some(true);
        assert_eq!(summary(&o), "Nothing needs you on nas01.");
    }

    #[test]
    fn needs_you_lists_failures_with_their_logs() {
        let html = overview_body(&overview()).into_string();
        assert!(html.contains(r#"href="/containers/radarr.container/logs""#));
        assert!(html.contains("exit 1 · 2h ago"));
        let mut quiet = overview();
        quiet.failed.clear();
        quiet.stats.failed = 0;
        assert!(
            overview_body(&quiet)
                .into_string()
                .contains("Nothing right now.")
        );
    }

    #[test]
    fn pretty_name_reads_os_release() {
        let os = "NAME=Fedora\nPRETTY_NAME=\"Fedora Linux 43 (Server)\"\nID=fedora\n";
        assert_eq!(pretty_name(os).as_deref(), Some("Fedora Linux 43 (Server)"));
        assert_eq!(pretty_name("ID=x\n"), None);
    }

    #[test]
    fn activity_marks_problems_and_reads_relative_time() {
        let entries = vec![Entry {
            at: 1_000,
            kind: Kind::Problem,
            subject: "radarr".into(),
            text: "exited with status 1".into(),
        }];
        let html = activity_items(&entries, 1_000 + 3 * 3600).into_string();
        assert!(html.contains("act-problem"));
        assert!(html.contains("3h ago"));
        assert!(activity_items(&[], 0).into_string().contains("Nothing yet"));
    }
}
