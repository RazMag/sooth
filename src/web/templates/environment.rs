use maud::{Markup, html};

use super::{BannerKind, NavItem, banner, csrf_input, page_header, shell};
use crate::health::Health;
use crate::hostenv::EnvVar;

/// Parameters for [`page`]. Grouped into a struct once `health` pushed the
/// plain argument list past clippy's `too_many_arguments` threshold.
pub struct EnvironmentPage<'a> {
    pub csrf: &'a str,
    pub configured: &'a [EnvVar],
    pub live: &'a [(String, String)],
    pub managed_file: &'a str,
    pub prefill: Option<(&'a str, &'a str)>,
    pub notice: Option<&'a str>,
    pub error: Option<&'a str>,
    pub health: Health,
}

/// The Environment page: an "add variable" form, the variables sooth manages
/// (editable), and any others configured under `~/.config/environment.d/`
/// (read-only). The inherited base environment is intentionally not shown.
pub fn page(p: EnvironmentPage<'_>) -> Markup {
    let EnvironmentPage {
        csrf,
        configured,
        live,
        managed_file,
        prefill,
        notice,
        error,
        health,
    } = p;
    let (pf_name, pf_value) = prefill.unwrap_or(("", ""));
    let managed: Vec<&EnvVar> = configured.iter().filter(|v| v.managed).collect();
    let external: Vec<&EnvVar> = configured.iter().filter(|v| !v.managed).collect();

    let body = html! {
        (page_header("Environment", html! {}))
        p.page-meta {
            "Host variables the systemd user manager passes to the quadlet generator. "
            "Reference any of them in a quadlet file as " code { "${NAME}" }
            " — it is expanded when the unit is (re)loaded."
        }

        @if let Some(msg) = notice { (banner(BannerKind::Success, msg)) }
        @if let Some(msg) = error { (banner(BannerKind::Error, msg)) }

        form.card method="post" action="/environment" {
            (csrf_input(csrf))
            div.field {
                label for="env-name" { "Name" }
                input.input #env-name type="text" name="name" value=(pf_name)
                    autocomplete="off" autocapitalize="off" spellcheck="false"
                    placeholder="app_path" required;
            }
            div.field {
                label for="env-value" { "Value" }
                input.input #env-value type="text" name="value" value=(pf_value)
                    autocomplete="off" spellcheck="false" placeholder="/srv/app";
            }
            button.btn.btn-primary type="submit" { "Add variable" }
        }

        h2 { "Managed by sooth" }
        @if managed.is_empty() {
            p.empty { "No variables set yet — add one above." }
        } @else {
            div.table-wrap {
                table.data-table {
                    thead { tr { th { "Reference" } th { "Value" } th {} } }
                    tbody {
                        @for v in &managed {
                            tr {
                                td { code { "${" (v.name) "}" } }
                                td { (value_cell(v, live)) }
                                td {
                                    form.inline-form method="post" action="/environment/delete"
                                        onsubmit="return confirm('Remove this variable?')" {
                                        (csrf_input(csrf))
                                        input type="hidden" name="name" value=(v.name);
                                        button.btn.btn-danger.btn-sm type="submit" { "Remove" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        @if !external.is_empty() {
            h2 { "Set elsewhere" }
            p.page-meta {
                "Defined in another file under " code { "~/.config/environment.d/" }
                " — edit that file directly."
            }
            div.table-wrap {
                table.data-table {
                    thead { tr { th { "Reference" } th { "Value" } th { "File" } } }
                    tbody {
                        @for v in &external {
                            tr {
                                td { code { "${" (v.name) "}" } }
                                td { (value_cell(v, live)) }
                                td { (v.source) }
                            }
                        }
                    }
                }
            }
        }

        p.field-hint {
            "Stored in " code { (managed_file) } ". Changes are applied to the running user "
            "manager immediately and re-read on your next login."
        }
    };
    shell(
        "Environment",
        Some(NavItem::Environment),
        Some(health),
        body,
    )
}

/// The value column: the live manager value when it has one, otherwise the
/// value from the file with a "pending" marker. When the two disagree, both
/// are shown so a stale live value is obvious.
fn value_cell(v: &EnvVar, live: &[(String, String)]) -> Markup {
    let live_val = live
        .iter()
        .find(|(k, _)| *k == v.name)
        .map(|(_, val)| val.as_str());
    html! {
        @match live_val {
            Some(val) if val == v.value => { @if val.is_empty() { span.muted { "—" } } @else { code { (val) } } }
            Some(val) => {
                @if val.is_empty() { span.muted { "—" } } @else { code { (val) } }
                div.cell-secondary { "file: " (v.value) }
            }
            None => {
                @if v.value.is_empty() { span.muted { "—" } } @else { code { (v.value) } }
                " "
                span.chip.chip-muted title="Not in the running manager yet — applies on next login" { "pending" }
            }
        }
    }
}
