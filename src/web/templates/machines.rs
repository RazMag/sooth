//! The Machines page: the machines this hub is paired with, and the Add
//! machine flow (address and code, then confirm the fingerprint).

use maud::{Markup, html};

use super::{BannerKind, Icon, NavItem, banner, csrf_input, icon, page_header, shell};
use crate::fleet::MachineEntry;
use crate::health::Health;
use crate::machine::Facts;
use crate::machine::api::Hello;

/// How a paired machine answered just now.
pub enum Reach {
    Online { facts: Facts, millis: u128 },
    Offline { reason: String },
}

pub struct Row {
    pub entry: MachineEntry,
    pub reach: Reach,
}

pub struct MachinesPage<'a> {
    pub csrf: &'a str,
    pub hub_name: &'a str,
    pub hub_fingerprint: String,
    pub rows: &'a [Row],
    /// Kept in the Add machine form after a failed attempt.
    pub address: &'a str,
    pub notice: Option<Markup>,
    pub error: Option<&'a str>,
    pub health: Health,
}

pub fn machines_page(p: MachinesPage<'_>) -> Markup {
    let body = html! {
        (page_header("Machines", html! {
            a.btn.btn-sm.btn-primary href="#add" { (icon(Icon::Plus)) span { "Add machine" } }
        }))
        p.page-meta {
            "This hub: " strong { (p.hub_name) } " · key "
            code.fingerprint { (p.hub_fingerprint) }
        }
        @if let Some(msg) = p.notice { div.banner.banner-success { (msg) } }
        @if let Some(msg) = p.error { (banner(BannerKind::Error, msg)) }

        @if p.rows.is_empty() {
            p.empty { "No other machines yet. This hub manages its own machine; add another to manage it from here too." }
        } @else {
            div.table-wrap {
                table.data-table.machines-table {
                    thead { tr { th { "Name" } th { "Status" } th { "Address" } th { "Access" } th {} } }
                    tbody {
                        @for row in p.rows { (machine_row(row, p.csrf)) }
                    }
                }
            }
        }

        h2 #add { "Add machine" }
        form.card method="post" action="/machines/add" autocomplete="off" {
            (csrf_input(p.csrf))
            div.field {
                label for="machine-address" { "Address" }
                input.input #machine-address type="url" name="address" value=(p.address)
                    placeholder="https://192.168.1.20:8421" required
                    autocapitalize="off" spellcheck="false";
                p.field-hint {
                    "Where this hub reaches the machine's agent. The agent listens on "
                    code { "127.0.0.1:8421" } " unless " code { "SOOTH_AGENT_BIND_ADDR" }
                    " says otherwise."
                }
            }
            div.field {
                label for="machine-code" { "Pairing code" }
                input.input #machine-code type="text" name="code" placeholder="ABCD-EFGH-JKLM"
                    required autocapitalize="characters" spellcheck="false";
                p.field-hint {
                    "On the new machine, " code { "sooth --agent" } " prints one at its first start, and "
                    code { "sooth --pair-code" } " makes a fresh one (add " code { "--read-only" }
                    " to let this hub look but not act). A code works once, for 10 minutes."
                }
            }
            button.btn.btn-primary type="submit" { "Continue" }
        }
    };
    shell("Machines", Some(NavItem::Machines), Some(p.health), body)
}

fn machine_row(row: &Row, csrf: &str) -> Markup {
    let e = &row.entry;
    html! {
        tr id={"machine-" (e.id)} {
            td {
                strong { (e.name) }
                @if let Reach::Online { facts, .. } = &row.reach {
                    div.machine-sub {
                        (facts.hostname) " · " (facts.arch)
                        @if let Some(os) = &facts.os { " · " (os) }
                    }
                }
            }
            td {
                @match &row.reach {
                    Reach::Online { facts, millis } => {
                        span.badge.badge-running { "Online" }
                        " " span.muted { "sooth " (facts.sooth_version) " · " (millis) " ms" }
                    }
                    Reach::Offline { reason } => {
                        span.badge.badge-failed title=(reason) { "Offline" }
                        div.field-hint { (reason) }
                    }
                }
            }
            td { code { (e.address) } }
            td.nowrap { (e.permission.label()) }
            td.registry-row-actions {
                details.inline-rename {
                    summary.btn-icon.btn-icon-sm title="Rename" aria-label={"Rename " (e.name)} {
                        (icon(Icon::Edit))
                    }
                    form.inline-form method="post" action="/machines/rename" {
                        (csrf_input(csrf))
                        input type="hidden" name="id" value=(e.id);
                        input.input.input-sm type="text" name="name" value=(e.name) required maxlength="40";
                        button.btn.btn-sm type="submit" { "Save" }
                    }
                }
                form.inline-form method="post" action="/machines/remove"
                    onsubmit="return confirm('Unpair this machine? Its services keep running, but this hub stops managing it. Pairing again needs a new code from the machine.')" {
                    (csrf_input(csrf))
                    input type="hidden" name="id" value=(e.id);
                    button.btn-icon.btn-icon-sm.btn-icon-danger type="submit"
                        aria-label={"Unpair " (e.name)} title="Unpair" {
                        (icon(Icon::Trash))
                    }
                }
            }
        }
    }
}

pub struct ConfirmPage<'a> {
    pub csrf: &'a str,
    pub address: &'a str,
    pub code: &'a str,
    pub fingerprint: String,
    pub hello: &'a Hello,
    pub name: &'a str,
    pub error: Option<&'a str>,
    pub health: Health,
}

/// Step two of Add machine: the agent's key, to compare by eye with the one
/// the machine printed, and a name for it.
pub fn confirm_page(p: ConfirmPage<'_>) -> Markup {
    let body = html! {
        (super::back_link("/machines", "Machines"))
        (page_header("Check the machine's key", html! {}))
        @if let Some(msg) = p.error { (banner(BannerKind::Error, msg)) }
        div.card.fingerprint-card {
            p {
                "The agent at " code { (p.address) } " is " strong { (p.hello.hostname) }
                ", running sooth " (p.hello.sooth_version) ". Its key's fingerprint is:"
            }
            p.fingerprint-big { code { (p.fingerprint) } }
            p.field-hint {
                "Compare it with the " strong { "Fingerprint" } " line that "
                code { "sooth --pair-code" } " or the agent's startup log printed on that machine. "
                "If they differ, something between here and there is answering in its place: don't pair."
            }
        }
        form.card method="post" action="/machines/pair" autocomplete="off" {
            (csrf_input(p.csrf))
            input type="hidden" name="address" value=(p.address);
            input type="hidden" name="code" value=(p.code);
            input type="hidden" name="fingerprint" value=(p.fingerprint);
            div.field {
                label for="machine-name" { "Name" }
                input.input #machine-name type="text" name="name" value=(p.name) required maxlength="40";
                p.field-hint { "How this machine shows up in sooth. You can rename it later." }
            }
            div.form-actions {
                button.btn.btn-primary type="submit" { "The keys match, pair" }
                a.btn href="/machines" { "Cancel" }
            }
        }
    };
    shell("Add machine", Some(NavItem::Machines), Some(p.health), body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::Identity;
    use crate::link::trust::Permission;

    fn entry() -> MachineEntry {
        MachineEntry {
            id: "pi".into(),
            name: "Pi".into(),
            address: "https://10.0.0.5:8421".into(),
            fingerprint: Identity::generate().unwrap().fingerprint,
            permission: Permission::ReadOnly,
            added_at: 0,
        }
    }

    #[test]
    fn rows_show_reach_access_and_actions() {
        let rows = [Row {
            entry: entry(),
            reach: Reach::Offline {
                reason: "couldn't reach it".into(),
            },
        }];
        let html = machines_page(MachinesPage {
            csrf: "tok",
            hub_name: "hub01",
            hub_fingerprint: "ab12".into(),
            rows: &rows,
            address: "",
            notice: None,
            error: None,
            health: Health::default(),
        })
        .into_string();
        assert!(html.contains(r#"id="machine-pi""#));
        assert!(html.contains("Offline"));
        assert!(html.contains("Read-only"));
        assert!(html.contains(r#"action="/machines/remove""#));
        assert!(html.contains(r#"action="/machines/add""#));
        assert!(html.contains(r#"aria-current="page""#));
    }

    #[test]
    fn the_confirm_step_carries_the_fingerprint_it_showed() {
        let hello = Hello {
            hostname: "pi".into(),
            sooth_version: "2.0.0".into(),
            api_version: 1,
        };
        let html = confirm_page(ConfirmPage {
            csrf: "tok",
            address: "https://10.0.0.5:8421",
            code: "AAAA-BBBB-CCCC",
            fingerprint: "ab12 cd34".into(),
            hello: &hello,
            name: "pi",
            error: None,
            health: Health::default(),
        })
        .into_string();
        assert!(html.contains(r#"name="fingerprint" value="ab12 cd34""#));
        assert!(html.contains(r#"name="code" value="AAAA-BBBB-CCCC""#));
    }
}
