//! The Machine page: this host's facts, its health checks, and a tile per
//! resource list (the lists themselves are this page's tabs, see
//! `machine_tabs`). In Phase 1 there is one machine; the hub will have one
//! of these per machine.

use maud::{Markup, html};

use super::overview::MachineInfo;
use super::{NavItem, machine_tabs, settings, shell};
use crate::health::Health;

/// A tool sooth shells out to, and the version it reported (`None`: not
/// found on PATH, or it didn't answer).
pub struct Tool {
    pub name: &'static str,
    pub version: Option<String>,
    /// What's affected when it's missing.
    pub needed_for: &'static str,
}

/// How many units of each resource kind exist.
#[derive(Default)]
pub struct Counts {
    pub services: usize,
    pub volumes: usize,
    pub networks: usize,
    pub images: usize,
    pub all: usize,
}

pub struct MachinePage {
    pub machine: MachineInfo,
    pub quadlet_dir: String,
    pub tools: Vec<Tool>,
    pub counts: Counts,
    pub health: Health,
}

/// `podman version 5.4.0` / `git version 2.47.1` -> the version alone.
pub fn parse_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_string)
}

fn tile(label: &str, value: usize, href: &str) -> Markup {
    html! {
        a.stat-card href=(href) {
            div.stat-card-label { (label) }
            div.stat-card-value { (value) }
        }
    }
}

pub fn machine_page(p: &MachinePage) -> Markup {
    let m = &p.machine;
    let podman = p
        .tools
        .iter()
        .find(|t| t.name == "podman")
        .and_then(|t| t.version.as_deref());
    let body = html! {
        div.page-header {
            h1 { (m.hostname) }
            div.actions {
                a.btn.btn-sm.btn-primary href="/containers/new" { "New unit here" }
            }
        }
        p.page-meta {
            (m.arch)
            @if let Some(os) = &m.os { " · " (os) }
            @if let Some(v) = podman { " · podman " (v) }
            " · sooth " (env!("CARGO_PKG_VERSION"))
            " · " code { (p.quadlet_dir) }
        }
        (machine_tabs(Some(NavItem::Machine)))
        div.stat-grid.machine-tiles {
            (tile("Services", p.counts.services, NavItem::Services.href()))
            (tile("Volumes", p.counts.volumes, "/volumes"))
            (tile("Networks", p.counts.networks, "/networks"))
            (tile("Images & builds", p.counts.images, "/images"))
            (tile("All units", p.counts.all, "/units"))
        }
        div.machine-grid {
            section {
                h2.card-title { "Health" }
                (settings::health_card(p.health))
            }
            section {
                h2.card-title { "Tools" }
                div.card {
                    table.kv-table {
                        @for t in &p.tools {
                            tr {
                                td { (t.name) }
                                td {
                                    @match &t.version {
                                        Some(v) => span.badge.badge-running { (v) },
                                        None => {
                                            span.badge.badge-warn { "Not found" }
                                            p.field-hint { "Needed for " (t.needed_for) "." }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    shell(&m.hostname, Some(NavItem::Machine), Some(p.health), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_takes_the_first_number() {
        assert_eq!(
            parse_version("podman version 5.4.0\n").as_deref(),
            Some("5.4.0")
        );
        assert_eq!(
            parse_version("git version 2.47.1").as_deref(),
            Some("2.47.1")
        );
        assert_eq!(parse_version("nope"), None);
    }

    #[test]
    fn missing_tool_says_what_it_breaks() {
        let page = machine_page(&MachinePage {
            machine: MachineInfo {
                hostname: "nas01".into(),
                arch: "x86_64",
                os: None,
            },
            quadlet_dir: "/q".into(),
            tools: vec![Tool {
                name: "git",
                version: None,
                needed_for: "Git Sync",
            }],
            counts: Counts::default(),
            health: Health::default(),
        })
        .into_string();
        assert!(page.contains("Needed for Git Sync."));
        assert!(page.contains(r#"aria-current="page">Overview</a>"#));
    }
}
