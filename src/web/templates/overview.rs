//! The Overview home page: one sentence on how things are, four tiles, a
//! card per machine in scope, and "Needs you" -- the problems worth acting
//! on, each with the action that fixes it.

use maud::{Markup, html};

use super::scope::{ScopeChoice, scope_switch};
use super::services::Stats;
use super::{Icon, NavItem, icon, rough_span, shell};
use crate::activity::{Entry, Kind};
use crate::health::Health;

/// What the host says about itself, for the machine card.
pub struct MachineInfo {
    pub hostname: String,
    pub arch: String,
    /// `/etc/os-release`'s `PRETTY_NAME`, when readable.
    pub os: Option<String>,
}

impl From<&crate::machine::Facts> for MachineInfo {
    fn from(f: &crate::machine::Facts) -> Self {
        Self {
            hostname: f.hostname.clone(),
            arch: f.arch.clone(),
            os: f.os.clone(),
        }
    }
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
            arch: std::env::consts::ARCH.to_string(),
            os,
        }
    }
}

/// Two letters for a machine's avatar.
fn initials(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase()
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
    /// Where its logs are.
    pub logs: String,
    /// "exit 1 · 2h ago", as the status cell shows it.
    pub detail: String,
    /// The machine it's on, when that isn't this hub.
    pub machine: Option<String>,
}

impl FailedUnit {
    /// "radarr.container", or "radarr.container on Pi".
    fn label(&self) -> String {
        match &self.machine {
            Some(m) => format!("{} on {m}", self.name),
            None => self.name.clone(),
        }
    }
}

/// How a machine answered the page.
pub enum Answer {
    Live,
    /// It didn't; its numbers are what it last reported.
    Stale {
        ago: String,
        reason: String,
    },
    /// It didn't, and hasn't since the hub started.
    Unreachable {
        reason: String,
    },
}

/// One machine's card.
pub struct MachineCard {
    pub name: String,
    pub href: String,
    /// This hub's own machine.
    pub local: bool,
    /// Its architecture and OS, when it has said.
    pub info: Option<MachineInfo>,
    /// Its services, when it has ever listed them.
    pub stats: Option<Stats>,
    pub health: Option<Health>,
    pub answer: Answer,
}

impl MachineCard {
    fn answering(&self) -> bool {
        matches!(self.answer, Answer::Live)
    }

    fn linger_off(&self) -> bool {
        self.health.is_some_and(|h| h.linger_enabled == Some(false))
    }

    fn no_generator(&self) -> bool {
        self.health.is_some_and(|h| !h.podman_generator_found)
    }
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
    /// A card per machine in scope, this hub's first.
    pub machines: Vec<MachineCard>,
    /// Every machine's services together.
    pub stats: Stats,
    pub failed: Vec<FailedUnit>,
    pub missing_secrets: Vec<MissingSecret>,
    /// Containers pinned to an image with newer tags; `None` when this
    /// hub's own machine isn't in scope (tags are only checked here so far).
    pub newer_tags: Option<Vec<String>>,
    pub sync: SyncSummary,
    /// Running containers whose podman health check is failing.
    pub unhealthy: Vec<FailedUnit>,
    /// The Services page, in the same scope.
    pub services_href: String,
}

impl Overview {
    fn newer_tags(&self) -> &[String] {
        self.newer_tags.as_deref().unwrap_or_default()
    }

    fn silent(&self) -> impl Iterator<Item = &MachineCard> {
        self.machines.iter().filter(|m| !m.answering())
    }

    /// " on Pi", when there's more than one machine to tell apart.
    fn on(&self, card: &MachineCard) -> String {
        if self.machines.len() > 1 {
            format!(" on {}", card.name)
        } else {
            String::new()
        }
    }
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
    // Each part, and whether it starts with a name (left as it is) rather
    // than a word to capitalise.
    let mut parts: Vec<(String, bool)> = Vec::new();
    match o.failed.as_slice() {
        [] => {}
        [one] => parts.push((format!("{} failed", one.label()), true)),
        many => parts.push((
            format!("{} failed", plural(many.len(), "unit", "units")),
            false,
        )),
    }
    match o.unhealthy.as_slice() {
        [] => {}
        [one] => parts.push((format!("{} is unhealthy", one.label()), true)),
        many => parts.push((
            format!(
                "{} are unhealthy",
                plural(many.len(), "container", "containers")
            ),
            false,
        )),
    }
    let silent: Vec<&MachineCard> = o.silent().collect();
    match silent.as_slice() {
        [] => {}
        [one] => parts.push((format!("{} isn't answering", one.name), true)),
        many => parts.push((
            format!(
                "{} aren't answering",
                plural(many.len(), "machine", "machines")
            ),
            false,
        )),
    }
    match o.missing_secrets.len() {
        0 => {}
        1 => parts.push((
            format!("secret {} isn't set", o.missing_secrets[0].name),
            false,
        )),
        n => parts.push((format!("{n} secrets aren't set"), false)),
    }
    for m in o.machines.iter().filter(|m| m.linger_off()) {
        parts.push((format!("linger is off{}", o.on(m)), false));
    }
    if o.sync.attention > 0 {
        parts.push((
            format!(
                "{} attention",
                plural(
                    o.sync.attention,
                    "Git Sync repo needs",
                    "Git Sync repos need"
                )
            ),
            false,
        ));
    }
    let Some(starts_with_name) = parts.first().map(|(_, name)| *name) else {
        return match o.machines.as_slice() {
            [one] => format!("Nothing needs you on {}.", one.name),
            _ => "Nothing needs you on any machine.".to_string(),
        };
    };
    let mut parts: Vec<String> = parts.into_iter().map(|(p, _)| p).collect();
    let last = parts.pop().unwrap_or_default();
    let mut sentence = if parts.is_empty() {
        last
    } else {
        format!("{} and {last}", parts.join(", "))
    };
    // Sentence case, without touching a name that starts it.
    if !starts_with_name && let Some(first) = sentence.get(..1) {
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
    let services = o.services_href.as_str();
    html! {
        div.stat-grid.overview-tiles {
            (tile("Running", o.stats.running.to_string(),
                html! { "of " (o.stats.total) }, services, false))
            (tile("Failed", o.stats.failed.to_string(), html! {
                @match o.failed.as_slice() {
                    [] => "none",
                    [one] => (one.label()),
                    many => (plural(many.len(), "unit", "units")),
                }
            }, services, o.stats.failed > 0))
            @match &o.newer_tags {
                Some(tags) => (tile("Newer image tags", tags.len().to_string(),
                    html! { (if tags.len() == 1 { "pinned image" } else { "pinned images" }) },
                    NavItem::ImageTags.href(), false)),
                None => (tile("Newer image tags", "–".to_string(),
                    html! { "checked on this hub only" },
                    NavItem::ImageTags.href(), false)),
            }
            (tile("Git Sync", o.sync.repos.to_string(), html! {
                (if o.sync.repos == 1 { "repo" } else { "repos" })
                @if o.sync.attention > 0 { " · " (o.sync.attention) " " (if o.sync.attention == 1 { "needs" } else { "need" }) " attention" }
            }, NavItem::GitSync.href(), o.sync.attention > 0))
        }
    }
}

fn machine_card(card: &MachineCard) -> Markup {
    let answering = card.answering();
    html! {
        section.card.machine-card.is-silent[!answering] {
            div.machine-head {
                span.machine-avatar aria-hidden="true" { (initials(&card.name)) }
                div {
                    a.machine-name href=(card.href) { (card.name) }
                    @if !card.local {
                        " " span.machine-dot.is-online[answering].is-offline[!answering] aria-hidden="true" {}
                    }
                    div.machine-meta {
                        @match &card.info {
                            Some(info) => {
                                (info.arch)
                                @if let Some(os) = &info.os { " · " (os) }
                            }
                            None => span.muted { "Not heard from yet" },
                        }
                    }
                }
            }
            @if let Some(stats) = &card.stats {
                @let stopped = stats.total - stats.running - stats.failed;
                div.machine-counts {
                    span.count-ok { (stats.running) " up" }
                    @if stats.failed > 0 { " · " span.count-failed { (stats.failed) " failed" } }
                    @if stopped > 0 { " · " (stopped) " off" }
                }
            }
            @match &card.answer {
                Answer::Live => {},
                Answer::Stale { ago, reason } => div.machine-warn {
                    "Not answering (" (reason) "). Numbers from " (ago) "."
                },
                Answer::Unreachable { reason } => div.machine-warn {
                    "Not answering (" (reason) ")."
                },
            }
            @if card.linger_off() {
                div.machine-warn {
                    "Linger is off: units stop at logout. "
                    @if card.local { a href=(NavItem::Settings.href()) { "Fix" } }
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
    let newer_tags = o.newer_tags();
    let any = !o.failed.is_empty()
        || !o.unhealthy.is_empty()
        || o.silent().next().is_some()
        || !o.missing_secrets.is_empty()
        || !newer_tags.is_empty()
        || o.sync.attention > 0
        || o.machines.iter().any(MachineCard::no_generator);
    html! {
        section.card.needs-you {
            h2.card-title { "Needs you" }
            @if !any {
                p.muted { "Nothing right now." }
            } @else {
                ul.need-list {
                    @for f in &o.failed {
                        (need(
                            html! {
                                strong { (f.name) } " failed"
                                @if let Some(m) = &f.machine { " on " (m) }
                            },
                            html! { (f.detail) },
                            html! {
                                a.btn.btn-sm href=(f.logs) { (icon(Icon::Logs)) span { "Logs" } }
                                a.btn.btn-sm href=(f.href) { "Open" }
                            },
                        ))
                    }
                    @for u in &o.unhealthy {
                        (need(
                            html! { strong { (u.name) } " is unhealthy" },
                            html! { "Running, but its health check is failing" },
                            html! {
                                a.btn.btn-sm href=(u.logs) { (icon(Icon::Logs)) span { "Logs" } }
                            },
                        ))
                    }
                    @for m in o.silent() {
                        (need(
                            html! { strong { (m.name) } " isn't answering" },
                            html! {
                                @match &m.answer {
                                    Answer::Stale { ago, reason } => { (reason) " · last answered " (ago) },
                                    Answer::Unreachable { reason } => { (reason) },
                                    Answer::Live => {},
                                }
                            },
                            html! { a.btn.btn-sm href="/machines" { "Machines" } },
                        ))
                    }
                    @for s in &o.missing_secrets {
                        (need(
                            html! { "Secret " code { (s.name) } " not set" },
                            html! { "Referenced by " (plural(s.units, "unit", "units")) },
                            html! { a.btn.btn-sm href=(NavItem::Secrets.href()) { "Set secret" } },
                        ))
                    }
                    @if !newer_tags.is_empty() {
                        (need(
                            html! { (plural(newer_tags.len(), "pinned image has", "pinned images have")) " newer tags" },
                            html! { (newer_tags.join(", ")) },
                            html! { a.btn.btn-sm href=(NavItem::ImageTags.href()) { "Review tags" } },
                        ))
                    }
                    @if o.sync.attention > 0 {
                        (need(
                            html! { (plural(o.sync.attention, "Git Sync repo needs", "Git Sync repos need")) " attention" },
                            html! { "The last sync failed or the checkout diverged" },
                            html! { a.btn.btn-sm href=(NavItem::GitSync.href()) { "Open Git Sync" } },
                        ))
                    }
                    @for m in o.machines.iter().filter(|m| m.no_generator()) {
                        (need(
                            html! { "Podman's quadlet generator wasn't found" (o.on(m)) },
                            html! { "Units won't start until Podman is installed" },
                            html! {
                                @if m.local {
                                    a.btn.btn-sm href=(NavItem::Settings.href()) { "Open Settings" }
                                }
                            },
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
            div.machine-cards {
                @for card in &o.machines { (machine_card(card)) }
            }
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

pub struct OverviewPage<'a> {
    pub overview: &'a Overview,
    pub activity: &'a [Entry],
    /// Unix seconds.
    pub now: u64,
    /// The scope switcher, when the hub has other machines.
    pub scopes: Option<Vec<ScopeChoice>>,
    /// `/overview/body` in the page's scope.
    pub body_route: &'a str,
    /// What redraws the body.
    pub refresh: &'a str,
    /// Offer New unit: this hub's own machine is in scope.
    pub creates: bool,
    pub health: Health,
}

pub fn overview_page(p: &OverviewPage) -> Markup {
    let o = p.overview;
    let body = html! {
        div.page-header.overview-header {
            h1 id="overview-headline" { (headline(&o.stats)) }
            div.actions {
                @if p.creates {
                    a.btn.btn-sm.btn-primary href="/containers/new" { (icon(Icon::Plus)) span { "New unit" } }
                }
            }
        }
        @if let Some(choices) = &p.scopes { (scope_switch(choices)) }
        div id="overview-body" hx-get=(p.body_route) hx-trigger=(p.refresh) hx-swap="innerHTML" {
            (overview_body(o))
        }
        (activity_card(p.activity, p.now))
    };
    shell(
        &headline(&o.stats),
        Some(NavItem::Overview),
        Some(p.health),
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, local: bool) -> MachineCard {
        MachineCard {
            name: name.into(),
            href: "/machine".into(),
            local,
            info: Some(MachineInfo {
                hostname: name.into(),
                arch: "x86_64".into(),
                os: Some("Fedora Linux 43".into()),
            }),
            stats: Some(Stats {
                total: 14,
                running: 12,
                failed: 1,
            }),
            health: Some(Health {
                podman_generator_found: true,
                linger_enabled: Some(true),
            }),
            answer: Answer::Live,
        }
    }

    fn overview() -> Overview {
        Overview {
            machines: vec![card("nas01", true)],
            stats: Stats {
                total: 14,
                running: 12,
                failed: 1,
            },
            failed: vec![FailedUnit {
                name: "radarr.container".into(),
                href: "/containers/radarr.container".into(),
                logs: "/containers/radarr.container/logs".into(),
                detail: "exit 1 · 2h ago".into(),
                machine: None,
            }],
            missing_secrets: vec![],
            newer_tags: Some(vec![]),
            sync: SyncSummary::default(),
            unhealthy: vec![],
            services_href: "/services".into(),
        }
    }

    fn set_linger(o: &mut Overview, on: bool) {
        if let Some(h) = &mut o.machines[0].health {
            h.linger_enabled = Some(on);
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
        set_linger(&mut o, false);
        assert_eq!(
            summary(&o),
            "radarr.container failed, secret db-pass isn't set and linger is off."
        );
        o.failed.clear();
        o.unhealthy.push(FailedUnit {
            name: "db.container".into(),
            href: String::new(),
            logs: String::new(),
            detail: String::new(),
            machine: None,
        });
        assert!(summary(&o).starts_with("db.container is unhealthy, secret"));
        o.unhealthy.clear();
        o.missing_secrets.clear();
        assert_eq!(summary(&o), "Linger is off.");
        set_linger(&mut o, true);
        assert_eq!(summary(&o), "Nothing needs you on nas01.");
    }

    #[test]
    fn other_machines_are_named_in_the_summary_and_needs_you() {
        let mut o = overview();
        o.failed[0].machine = Some("Pi".into());
        o.failed[0].logs = "/machines/pi/units/radarr.container#logs".into();
        let mut pi = card("Pi", false);
        pi.answer = Answer::Stale {
            ago: "3m ago".into(),
            reason: "no answer within 4 seconds".into(),
        };
        o.machines.push(pi);
        assert_eq!(
            summary(&o),
            "radarr.container on Pi failed and Pi isn't answering."
        );
        let html = overview_body(&o).into_string();
        assert!(html.contains("isn't answering"));
        assert!(html.contains("last answered 3m ago"));
        assert!(html.contains(r#"href="/machines/pi/units/radarr.container#logs""#));
        assert!(html.contains("Numbers from 3m ago"));

        o.failed.clear();
        o.machines[1].answer = Answer::Live;
        assert_eq!(summary(&o), "Nothing needs you on any machine.");
        o.machines[1].health = Some(Health {
            podman_generator_found: true,
            linger_enabled: Some(false),
        });
        assert_eq!(summary(&o), "Linger is off on Pi.");
    }

    #[test]
    fn newer_tags_say_when_they_werent_checked() {
        let mut o = overview();
        o.newer_tags = None;
        assert!(tiles(&o).into_string().contains("checked on this hub only"));
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
