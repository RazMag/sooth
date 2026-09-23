use std::collections::BTreeMap;

use maud::{Markup, html};

use super::{BannerKind, Icon, NavItem, banner, csrf_input, icon, page_header, shell, unit_links};
use crate::health::Health;
use crate::quadlet::QuadletUnit;
use crate::secrets::SecretInfo;

/// Parameters for [`page`] -- same grouping as `environment::EnvironmentPage`.
pub struct SecretsPage<'a> {
    pub csrf: &'a str,
    pub secrets: &'a [SecretInfo],
    /// File names consuming each secret, parallel to `secrets`.
    pub used_by: &'a [Vec<String>],
    /// Referenced by some quadlet but not in podman's store.
    pub missing: &'a BTreeMap<String, Vec<String>>,
    pub all_units: &'a [QuadletUnit],
    /// Name only -- a rejected value is never echoed back into the page.
    pub prefill_name: &'a str,
    pub notice: Option<Markup>,
    pub error: Option<&'a str>,
    pub health: Health,
}

/// The Secrets page: podman secrets (write-only values), which units consume
/// each one, and any `Secret=` references that don't resolve yet -- the
/// usual case right after git-syncing a repo that names secrets it can't
/// carry.
pub fn page(p: SecretsPage<'_>) -> Markup {
    let SecretsPage {
        csrf,
        secrets,
        used_by,
        missing,
        all_units,
        prefill_name,
        notice,
        error,
        health,
    } = p;

    let body = html! {
        (page_header("Secrets", html! {}))
        p.page-meta {
            "Values stored in podman's secret store. Reference one from a container quadlet as "
            code { "Secret=NAME,type=env,target=VAR" } " (or " code { "type=mount" }
            ") — only the name goes in the quadlet file, so it's safe to commit to a "
            "git-synced repo. Values stay hidden until you click the eye."
        }

        @if let Some(msg) = notice { div.banner.banner-success { (msg) } }
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }

        @if !missing.is_empty() {
            h2 { "Missing" }
            p.page-meta {
                "Referenced by a quadlet but not set yet — those units will fail to start "
                "until they are."
            }
            div.table-wrap {
                table.data-table.secrets-table {
                    thead { tr { th { "Name" } th { "Value" } th { "Used by" } } }
                    tbody {
                        @for (name, users) in missing {
                            tr id={"secret-" (name)} {
                                td { code { (name) } }
                                td {
                                    div.secret-value-line {
                                        span.secret-mask.muted { "not set" }
                                        @if crate::secrets::valid_name(name) {
                                            (value_dialog_button(name, "set", users.len()))
                                        } @else {
                                            span.muted title="Not a valid podman secret name — fix the quadlet" { "invalid name" }
                                        }
                                    }
                                }
                                td { (unit_links(all_units, users)) }
                            }
                        }
                    }
                }
            }
        }

        h2 { "Add a secret" }
        form.card method="post" action="/secrets" autocomplete="off" {
            (csrf_input(csrf))
            div.field {
                label for="secret-name" { "Name" }
                input.input #secret-name type="text" name="name" value=(prefill_name)
                    autocomplete="off" autocapitalize="off" spellcheck="false"
                    placeholder="db-password" required;
            }
            div.field {
                label for="secret-value" { "Value" }
                textarea.input #secret-value name="value" autocomplete="off"
                    spellcheck="false" required {}
            }
            button.btn.btn-primary type="submit" { "Add secret" }
        }

        h2 { "Stored" }
        @if secrets.is_empty() {
            p.empty { "No secrets in podman's store yet." }
        } @else {
            div.table-wrap {
                table.data-table.secrets-table data-secrets-csrf=(csrf) {
                    thead { tr { th { "Name" } th { "Value" } th { "Used by" } th { "Driver" } th { "Updated" } th {} } }
                    tbody {
                        @for (s, users) in secrets.iter().zip(used_by) {
                            @let valid = crate::secrets::valid_name(&s.name);
                            tr id={"secret-" (s.name)} {
                                td { code { (s.name) } }
                                td {
                                    div.secret-value-line data-secret=(s.name) {
                                        span.secret-mask aria-hidden="true" { "••••••••" }
                                        code.secret-plain hidden {}
                                        @if valid {
                                            button.btn-icon.btn-icon-sm type="button" data-secret-toggle
                                                aria-pressed="false" aria-label={"Show value of " (s.name)} title="Show value" {
                                                span.icon-when-hidden { (icon(Icon::Eye)) }
                                                span.icon-when-shown hidden { (icon(Icon::EyeOff)) }
                                            }
                                            button.btn-icon.btn-icon-sm.secret-copy-off type="button" data-secret-copy
                                                tabindex="-1" aria-hidden="true"
                                                aria-label={"Copy value of " (s.name)} title="Copy value" {
                                                span.icon-copy { (icon(Icon::Copy)) }
                                                span.icon-copied hidden { (icon(Icon::Check)) }
                                            }
                                            (value_dialog_button(&s.name, "replace", users.len()))
                                        }
                                    }
                                }
                                td { (unit_links(all_units, users)) }
                                td { (s.driver) }
                                td { (s.updated) }
                                td.secret-row-actions {
                                    @if valid && !users.is_empty() {
                                        form.inline-form method="post" action="/secrets/restart" {
                                            (csrf_input(csrf))
                                            input type="hidden" name="name" value=(s.name);
                                            button.btn-icon.btn-icon-sm type="submit"
                                                aria-label={"Restart units using " (s.name)}
                                                title={"Restart the " (unit_count(users.len())) " using it (running and failed only)"} {
                                                (icon(Icon::Restart))
                                            }
                                        }
                                    }
                                    @if users.is_empty() {
                                        form.inline-form method="post" action="/secrets/delete"
                                            onsubmit="return confirm('Delete this secret? This cannot be undone.')" {
                                            (csrf_input(csrf))
                                            input type="hidden" name="name" value=(s.name);
                                            button.btn-icon.btn-icon-sm.btn-icon-danger type="submit"
                                                aria-label={"Delete " (s.name)} title="Delete" {
                                                (icon(Icon::Trash))
                                            }
                                        }
                                    } @else {
                                        button.btn-icon.btn-icon-sm type="button" disabled
                                            aria-label={"Delete " (s.name)}
                                            title="In use — remove the Secret= lines referencing it first" {
                                            (icon(Icon::Trash))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        (value_dialog(csrf))

        p.field-hint {
            "A container reads its secrets when it starts, so a new value only takes effect "
            "once the units using it restart — \"Replace\" can do that for you. Restarts "
            "touch running and failed units only; stopped ones stay stopped. Podman's default " code { "file" } " driver keeps "
            "values unencrypted (owner-only) under " code { "~/.local/share/containers/storage/secrets" }
            "; configure another driver in " code { "containers.conf" } " for encryption at rest."
        }
    };
    shell("Secrets", Some(NavItem::Secrets), Some(health), body)
}

/// The "Set"/"Replace" button in a Value cell. Opens the page's one shared
/// [`value_dialog`], which `frontend/secrets.js` fills in from these data
/// attributes -- keeps the row a single line however long the form is.
fn value_dialog_button(name: &str, mode: &str, users: usize) -> Markup {
    html! {
        button.btn.btn-sm type="button" data-secret-edit=(name) data-mode=(mode) data-users=(users) {
            @if mode == "replace" { "Replace" } @else { "Set" }
        }
    }
}

/// The shared Set/Replace modal. `replace` and the restart checkbox are
/// enabled/disabled per opening by `frontend/secrets.js`; a disabled input
/// isn't submitted, so a "Set" never posts `replace`.
fn value_dialog(csrf: &str) -> Markup {
    html! {
        dialog.modal-dialog data-secret-dialog {
            div.modal-header {
                h2 { span data-secret-dialog-title { "Replace" } " " code data-secret-dialog-name {} }
                button.btn-icon type="button" data-secret-dialog-close aria-label="Close" {
                    (icon(Icon::X))
                }
            }
            form.modal-body method="post" action="/secrets" autocomplete="off" {
                (csrf_input(csrf))
                input type="hidden" name="name" value="";
                input type="hidden" name="replace" value="1";
                div.field {
                    label for="secret-dialog-value" { "New value" }
                    textarea.input #secret-dialog-value name="value" autocomplete="off"
                        spellcheck="false" required {}
                }
                div.field data-secret-dialog-restart {
                    label.checkbox-line {
                        input type="checkbox" name="restart" value="1" checked;
                        span data-secret-dialog-restart-label { "Restart the units using it" }
                    }
                    p.field-hint { "Running and failed units only — stopped ones stay stopped." }
                }
                div.modal-actions {
                    button.btn.btn-ghost type="button" data-secret-dialog-close { "Cancel" }
                    button.btn.btn-primary type="submit" { "Save" }
                }
            }
        }
    }
}

/// "Restart N units" -- `POST /secrets/restart`, offered in the notice after
/// a replace that didn't restart anything.
pub fn restart_button(csrf: &str, name: &str, users: usize) -> Markup {
    html! {
        form.inline-form method="post" action="/secrets/restart" {
            (csrf_input(csrf))
            input type="hidden" name="name" value=(name);
            button.btn.btn-sm type="submit"
                title="Restarts running and failed units that use this secret" {
                "Restart " (unit_count(users))
            }
        }
    }
}

fn unit_count(n: usize) -> String {
    if n == 1 {
        "1 unit".into()
    } else {
        format!("{n} units")
    }
}

/// The secret chips for the editor's insert panel (`host_vars_panel`),
/// lazy-loaded from `GET /secrets/chips` so the dozen editor call sites don't
/// each need a podman round trip threaded through them.
pub fn chips(names: &[String]) -> Markup {
    html! {
        @if names.is_empty() {
            p.field-hint {
                "None stored. " a href="/secrets" { "Add a secret" }
                " to reference it here."
            }
        } @else {
            p.field-hint {
                "Click a name to insert a " code { "Secret=" } " line (exposed as an "
                "environment variable of the same name, upper-cased) at the cursor."
            }
            div.host-vars-grid {
                @for name in names {
                    button.chip type="button"
                        data-insert-ref={"Secret=" (name) ",type=env,target=" (env_target(name))} {
                        (name)
                    }
                    span.host-vars-val { span.muted { "secret" } }
                }
            }
        }
    }
}

/// A plausible env var name for a secret: upper-cased, with `-`/`.` turned
/// into `_` (secret names allow both; env names don't).
fn env_target(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '-' | '.' => '_',
            c => c.to_ascii_uppercase(),
        })
        .collect()
}

/// The "N secrets missing" row for a git-sync card (see `gitsync::rows`).
pub fn missing_row(missing: &BTreeMap<String, Vec<String>>) -> Markup {
    html! {
        @if !missing.is_empty() {
            tr {
                td { "Secrets" }
                td {
                    span.badge.badge-warn { (missing.len()) " missing" }
                    " "
                    @for (i, name) in missing.keys().enumerate() {
                        @if i > 0 { ", " }
                        a href={"/secrets#secret-" (name)} { (name) }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::secrets::SecretInfo;

    fn secret(name: &str) -> SecretInfo {
        SecretInfo {
            name: name.into(),
            driver: "file".into(),
            created: "now".into(),
            updated: "now".into(),
        }
    }

    fn render(
        secrets: &[SecretInfo],
        used_by: &[Vec<String>],
        missing: &BTreeMap<String, Vec<String>>,
    ) -> String {
        page(SecretsPage {
            csrf: "tok",
            secrets,
            used_by,
            missing,
            all_units: &[],
            prefill_name: "",
            notice: None,
            error: None,
            health: Health::default(),
        })
        .into_string()
    }

    #[test]
    fn stored_row_masks_value_and_keeps_controls_on_one_line() {
        let html = render(&[secret("db-pass")], &[vec![]], &BTreeMap::new());
        // Masked, with an empty slot for the value -- nothing is revealed
        // until the eye toggle fetches it.
        assert!(html.contains(r#"data-secret="db-pass""#));
        assert!(html.contains("••••••••"));
        assert!(html.contains(r#"<code class="secret-plain" hidden></code>"#));
        // One toggle (starts un-pressed), copy reserved-but-invisible, and
        // Replace, all inside the same value line.
        let line_start = html.find("secret-value-line").unwrap();
        let line_end = line_start + html[line_start..].find("</td>").unwrap();
        let line = &html[line_start..line_end];
        assert_eq!(line.matches("data-secret-toggle").count(), 1);
        assert!(line.contains(r#"aria-pressed="false""#));
        assert!(line.contains("secret-copy-off"));
        assert!(line.contains(r#"data-secret-edit="db-pass" data-mode="replace""#));
        // The page carries the CSRF token the toggle posts with.
        assert!(html.contains(r#"data-secrets-csrf="tok""#));
    }

    #[test]
    fn unused_secret_can_be_deleted_and_has_no_restart() {
        let html = render(&[secret("old")], &[vec![]], &BTreeMap::new());
        assert!(html.contains(r#"action="/secrets/delete""#));
        assert!(!html.contains(r#"action="/secrets/restart""#));
    }

    #[test]
    fn used_secret_offers_restart_and_blocks_delete() {
        let html = render(
            &[secret("db-pass")],
            &[vec!["web.container".into(), "db.container".into()]],
            &BTreeMap::new(),
        );
        assert!(html.contains(r#"action="/secrets/restart""#));
        assert!(html.contains("Restart the 2 units using it"));
        assert!(!html.contains(r#"action="/secrets/delete""#));
        assert!(html.contains(r#"data-users="2""#));
    }

    #[test]
    fn missing_secret_gets_a_set_button_invalid_name_does_not() {
        let missing = BTreeMap::from([
            ("api-key".to_string(), vec!["web.container".to_string()]),
            ("-bad".to_string(), vec!["x.container".to_string()]),
        ]);
        let html = render(&[], &[], &missing);
        assert!(html.contains(r#"id="secret-api-key""#));
        assert!(html.contains(r#"data-secret-edit="api-key" data-mode="set""#));
        assert!(!html.contains(r#"data-secret-edit="-bad""#));
        assert!(html.contains("invalid name"));
    }

    #[test]
    fn value_dialog_posts_to_set_with_restart_option() {
        let html = render(&[], &[], &BTreeMap::new());
        assert!(html.contains("data-secret-dialog"));
        assert!(html.contains(r#"name="replace" value="1""#));
        assert!(html.contains(r#"name="restart" value="1" checked"#));
    }

    #[test]
    fn missing_row_lists_names_or_renders_nothing() {
        assert!(missing_row(&BTreeMap::new()).into_string().is_empty());
        let missing = BTreeMap::from([
            ("a".to_string(), vec!["x.container".to_string()]),
            ("b".to_string(), vec!["y.container".to_string()]),
        ]);
        let html = missing_row(&missing).into_string();
        assert!(html.contains("2 missing"));
        assert!(html.contains(r#"href="/secrets#secret-a""#));
        assert!(html.contains(r#"href="/secrets#secret-b""#));
    }

    #[test]
    fn chips_insert_a_secret_line() {
        let html = chips(&["db-pass".to_string()]).into_string();
        assert!(html.contains(r#"data-insert-ref="Secret=db-pass,type=env,target=DB_PASS""#));
        assert!(chips(&[]).into_string().contains(r#"href="/secrets""#));
    }

    #[test]
    fn env_target_uppercases_and_replaces_separators() {
        assert_eq!(env_target("db-pass.v2"), "DB_PASS_V2");
    }
}
