//! A unit on another machine (`/machines/{id}/units/{file}`): its status,
//! start / stop / restart when the hub may act there, its quadlet's sections
//! and the end of its journal. Editing, moving and deleting a unit on another
//! machine come with the agent's write calls in a later update; this page
//! says so instead of offering them.
//!
//! Also the list rows' menu for such a unit ([`remote_kebab`]).

use maud::{Markup, html};

use super::list::{OnMachine, remote_unit_url};
use super::logs::log_lines;
use super::{
    BannerKind, Icon, NavItem, back_link, banner, csrf_input, icon, page_header,
    plain_status_badge, section_table, shell,
};
use crate::fleet::MachineEntry;
use crate::health::Health;
use crate::journal::LogLine;
use crate::link::trust::Permission;
use crate::quadlet::QuadletUnit;
use crate::systemd::UnitStatus;

/// How many journal lines the page shows.
pub const LOG_LINES: u32 = 200;

/// The actions offered for a unit in `status`: (route, label, icon, class).
fn actions_for(status: &UnitStatus) -> Vec<(&'static str, &'static str, Icon, &'static str)> {
    if status.is_active() {
        vec![
            ("stop", "Stop", Icon::Stop, "btn-stop"),
            ("restart", "Restart", Icon::Restart, "btn-restart"),
        ]
    } else {
        vec![("start", "Start", Icon::Play, "btn-start")]
    }
}

/// The list row's menu for another machine's unit: its actions when the hub
/// may run them now (each answers in the menu, then redraws the list), and
/// links to the unit's page.
pub fn remote_kebab(on: &OnMachine, unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let base = remote_unit_url(on.id, &unit.file_name);
    html! {
        details.menu {
            summary aria-label="Actions" { (icon(Icon::More)) }
            div.menu-panel {
                @if on.actions && !unit.is_template() {
                    div.menu-actions {
                        @for (action, label, ic, class) in actions_for(status) {
                            form.inline-form hx-post={(base) "/" (action) "?style=menu"}
                                hx-target="next .menu-result" hx-swap="innerHTML" {
                                (csrf_input(csrf))
                                button class={"btn btn-sm " (class)} type="submit" {
                                    (icon(ic)) span { (label) }
                                }
                            }
                        }
                    }
                    span.menu-result role="status" {}
                }
                a href=(base) { (icon(Icon::Info)) span { "Details" } }
                a href={(base) "#logs"} { (icon(Icon::Logs)) span { "Logs" } }
            }
        }
    }
}

/// The one-line answer an action gives in a list row's menu.
pub fn menu_result(outcome: Result<&str, &str>) -> Markup {
    html! {
        @match outcome {
            Ok(done) => span.menu-result-ok { (done) },
            Err(e) => span.menu-result-error { (e) },
        }
    }
}

/// Why the hub can't act on any unit there right now, if it can't.
fn blocked_reason(machine: &MachineEntry, live: bool) -> Option<&'static str> {
    if machine.permission == Permission::ReadOnly {
        Some("This hub has read-only access to this machine.")
    } else if !live {
        Some("The machine isn't answering, so its units can't be started or stopped from here.")
    } else {
        None
    }
}

/// Why the hub can't act on `unit` right now, if it can't.
pub fn blocked_for(machine: &MachineEntry, unit: &QuadletUnit, live: bool) -> Option<&'static str> {
    if unit.is_template() {
        Some("Template unit — managed read-only.")
    } else {
        blocked_reason(machine, live)
    }
}

/// The status and action buttons, re-rendered by each action.
pub fn live_block(
    machine: &MachineEntry,
    file_name: &str,
    status: &UnitStatus,
    blocked: Option<&str>,
    message: Option<Result<&str, &str>>,
    csrf: &str,
) -> Markup {
    let base = remote_unit_url(&machine.id, file_name);
    html! {
        div #remote-unit-live .detail-toolbar {
            (plain_status_badge(status))
            @if let Some(reason) = blocked {
                span.muted { (reason) }
            } @else {
                @for (action, label, ic, class) in actions_for(status) {
                    form.inline-form hx-post={(base) "/" (action)}
                        hx-target="#remote-unit-live" hx-swap="outerHTML" {
                        (csrf_input(csrf))
                        button class={"btn btn-sm " (class)} type="submit" {
                            (icon(ic)) span { (label) }
                        }
                    }
                }
            }
            @match message {
                Some(Ok(done)) => span.muted role="status" { (done) },
                Some(Err(e)) => span.action-error role="status" { (e) },
                None => {}
            }
        }
    }
}

/// Whether the page shows fresh data, or what it last heard.
pub enum Freshness<'a> {
    Live,
    Stale { ago: String, reason: &'a str },
}

pub struct RemoteUnitPage<'a> {
    pub machine: &'a MachineEntry,
    pub unit: &'a QuadletUnit,
    pub status: &'a UnitStatus,
    pub freshness: Freshness<'a>,
    /// The end of its journal, or why it couldn't be read.
    pub logs: Result<&'a [LogLine], String>,
    pub csrf: &'a str,
    pub health: Health,
}

const TABS: [(&str, &str); 3] = [
    ("overview", "Overview"),
    ("config", "Config"),
    ("logs", "Logs"),
];

pub fn remote_unit_page(p: RemoteUnitPage<'_>) -> Markup {
    let m = p.machine;
    let unit = p.unit;
    let live = matches!(p.freshness, Freshness::Live);
    let blocked = blocked_for(m, unit, live);
    let scoped = format!("/services?m={}", m.id);
    let body = html! {
        (back_link(&scoped, &m.name))
        (page_header(&unit.file_name, html! {
            a.chip.chip-machine href=(scoped) title="The machine this unit is on" {
                span.machine-dot.is-online[live].is-offline[!live] aria-hidden="true" {}
                (m.name)
            }
        }))
        @if let Freshness::Stale { ago, reason } = &p.freshness {
            div.banner.banner-warn {
                (m.name) " isn't answering (" (reason) "). This is what it reported " (ago) ". "
                a href="?retry" { "Retry" }
            }
        }
        (live_block(m, &unit.file_name, p.status, blocked, None, p.csrf))

        div.unit-tabs {
            @for (id, _) in TABS {
                input.tab-radio type="radio" name="unit-tab" id={"tab-" (id)} value=(id) checked[id == "overview"];
            }
            nav.tab-bar aria-label="Unit sections" {
                @for (id, label) in TABS {
                    label for={"tab-" (id)} data-tab=(id) { (label) }
                }
            }
            section.tab-panel data-panel="overview" {
                div.detail-grid.solo {
                    aside.detail-aside { div.card {
                        h2.card-title { "Overview" }
                        table.kv-table {
                            tr { td { "Machine" } td { (m.name) } }
                            tr { td { "Kind" } td { (unit.kind.primary_section()) } }
                            tr { td { "Service" } td { code { (unit.service_name()) } } }
                            tr { td { "File" } td { code { (unit.rel_path()) } } }
                            tr { td { "Access" } td { (m.permission.label()) } }
                        }
                    } }
                }
                p.field-hint.remote-unit-note {
                    "Editing, moving and deleting units on other machines comes in a later "
                    "2.0 update. For now, change the file on " (m.name) " itself."
                }
            }
            section.tab-panel data-panel="config" {
                div.config-head { code.page-meta { (unit.rel_path()) } }
                (section_table(unit))
            }
            section.tab-panel data-panel="logs" {
                div.logs-panel-head {
                    span.muted { "The last " (LOG_LINES) " lines, as of this page load." }
                    // A new query string, so the browser loads the page again
                    // and the hash reopens this tab.
                    a.btn.btn-ghost.btn-sm href="?logs#logs" { (icon(Icon::Refresh)) span { "Refresh" } }
                }
                @match &p.logs {
                    Ok(lines) => (log_lines(lines)),
                    Err(reason) => (banner(BannerKind::Info, &format!("Couldn't read its logs: {reason}"))),
                }
            }
        }
    };
    shell(
        &unit.file_name,
        Some(NavItem::Services),
        Some(p.health),
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::Identity;
    use crate::machine::UnitRecord;

    fn machine(permission: Permission) -> MachineEntry {
        MachineEntry {
            id: "pi".into(),
            name: "Pi".into(),
            address: "https://10.0.0.5:8421".into(),
            agent_port: None,
            fingerprint: Identity::generate().unwrap().fingerprint,
            permission,
            added_at: 0,
        }
    }

    fn unit() -> QuadletUnit {
        UnitRecord {
            file_name: "web.container".into(),
            group: String::new(),
            raw: "[Container]\nImage=nginx\n".into(),
            status: None,
        }
        .into_unit(std::path::Path::new("/"))
        .unwrap()
        .0
    }

    fn page(permission: Permission, freshness: Freshness) -> String {
        let m = machine(permission);
        let u = unit();
        remote_unit_page(RemoteUnitPage {
            machine: &m,
            unit: &u,
            status: &UnitStatus::not_found(),
            freshness,
            logs: Ok(&[]),
            csrf: "tok",
            health: Health::default(),
        })
        .into_string()
    }

    #[test]
    fn a_full_access_machine_that_answers_offers_actions_there() {
        let html = page(Permission::Full, Freshness::Live);
        assert!(html.contains(r#"hx-post="/machines/pi/units/web.container/start""#));
        assert!(html.contains("Image"), "its sections are shown");
        assert!(html.contains(r#"href="/services?m=pi""#));
        assert!(!html.contains("sse-swap"));
        assert!(!html.contains("/edit"));
    }

    #[test]
    fn read_only_or_offline_says_why_instead_of_offering_actions() {
        let html = page(Permission::ReadOnly, Freshness::Live);
        assert!(!html.contains("hx-post"));
        assert!(html.contains("read-only access"));

        let html = page(
            Permission::Full,
            Freshness::Stale {
                ago: "3m ago".into(),
                reason: "no answer within 4 seconds",
            },
        );
        assert!(!html.contains("hx-post"));
        assert!(html.contains("This is what it reported 3m ago"));
        assert!(html.contains("Retry"));
    }

    #[test]
    fn the_menu_answer_says_what_happened() {
        assert!(
            menu_result(Ok("Started."))
                .into_string()
                .contains("Started.")
        );
        assert!(
            menu_result(Err("read-only"))
                .into_string()
                .contains("menu-result-error")
        );
    }
}
