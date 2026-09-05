//! HTML rendering. Templates are plain Rust functions returning `maud::Markup`
//! rather than a separate template file format -- handlers frequently need to
//! return just a fragment (a status badge, an error banner) after an htmx
//! request, and composing small functions makes that natural.

use axum::http::StatusCode;
use maud::{html, Markup, DOCTYPE};
use uuid::Uuid;

use crate::quadlet::{QuadletUnit, UnitKind};
use crate::systemd::UnitStatus;

/// The page shell shared by every full-page response: doctype, head, nav.
fn shell(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · sooth" }
                link rel="stylesheet" href="/static/style.css";
                script src="/static/htmx.min.js" {}
                script src="/static/sse.js" {}
            }
            body hx-ext="sse" sse-connect="/events" {
                header.topbar {
                    a.brand href="/" { "sooth" }
                    nav {
                        a href="/" { "Dashboard" }
                        a href="/units/new" { "New unit" }
                        form method="post" action="/logout" style="display:inline" { button.link type="submit" { "Log out" } }
                    }
                }
                main { (body) }
            }
        }
    }
}

fn csrf_input(csrf: &str) -> Markup {
    html! { input type="hidden" name="csrf_token" value=(csrf); }
}

/// A systemd service name like `myapp.service` is not a valid bare CSS
/// identifier (the `.` reads as a class selector to `querySelector`, and
/// instance units can contain `@` too), so it can't be used directly in an
/// `id`/`hx-target` pair -- only in places matched by plain string equality
/// (SSE event names). This gives every unit a stable, selector-safe DOM id.
fn dom_id(service: &str) -> String {
    let safe: String =
        service.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    format!("status-{safe}")
}

/// The live-updating status badge for one unit. Rendered both inline in the
/// dashboard/detail pages and as the payload of `status-{service}` SSE
/// events, so both paths converge on identical markup.
pub fn status_badge(service: &str, status: &UnitStatus) -> Markup {
    let class = if status.is_failed() {
        "status status-failed"
    } else if status.is_active() {
        "status status-active"
    } else if status.load_state == "not-found" {
        "status status-unknown"
    } else {
        "status status-inactive"
    };
    html! {
        div id=(dom_id(service)) class=(class) sse-swap={"status-" (service)} hx-swap="outerHTML" {
            span.dot {}
            span.state { (status.active_state) "/" (status.sub_state) }
            @if status.is_enabled() {
                span.pill { "enabled" }
            }
        }
    }
}

/// `file_name` (the quadlet file, e.g. `testo.container`) drives the URL --
/// routes and handlers key everything off the file, not the systemd unit
/// name -- while `service` (e.g. `testo.service`) drives which status badge
/// gets the response swapped into it.
fn action_button(file_name: &str, service: &str, action: &str, label: &str, csrf: &str, extra_class: &str) -> Markup {
    html! {
        form.inline-action
            hx-post={"/units/" (file_name) "/" (action)}
            hx-target={"#" (dom_id(service))}
            hx-swap="outerHTML" {
            (csrf_input(csrf))
            button type="submit" class=(format!("btn {extra_class}")) { (label) }
        }
    }
}

fn actions(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let service = unit.service_name();
    let file_name = &unit.file_name;
    html! {
        div.actions {
            @if status.is_active() {
                (action_button(file_name, &service, "stop", "Stop", csrf, "btn-warn"))
                (action_button(file_name, &service, "restart", "Restart", csrf, ""))
            } @else {
                (action_button(file_name, &service, "start", "Start", csrf, "btn-primary"))
            }
            @if status.is_enabled() {
                (action_button(file_name, &service, "disable", "Disable", csrf, ""))
            } @else {
                (action_button(file_name, &service, "enable", "Enable", csrf, ""))
            }
        }
    }
}

fn unit_row(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let service = unit.service_name();
    html! {
        tr {
            td {
                a href={"/units/" (unit.file_name)} { (unit.file_name) }
                @if unit.is_template() {
                    span.pill.pill-muted title="Template unit -- managed read-only, use the CLI to instantiate it" { "template" }
                }
                @if let Some(desc) = unit.description() {
                    div.muted { (desc) }
                }
            }
            td { (unit.kind.primary_section()) }
            td { (service) }
            td { (status_badge(&service, status)) }
            td { @if !unit.is_template() { (actions(unit, status, csrf)) } }
        }
    }
}

pub fn unit_rows(units: &[(QuadletUnit, UnitStatus)], csrf: &str) -> Markup {
    html! {
        @if units.is_empty() {
            tr { td colspan="5" .empty { "No quadlet files found yet. " a href="/units/new" { "Create one" } "." } }
        } @else {
            @for (unit, status) in units {
                (unit_row(unit, status, csrf))
            }
        }
    }
}

pub fn dashboard_page(units: &[(QuadletUnit, UnitStatus)], csrf: &str, quadlet_dir: &str) -> Markup {
    let body = html! {
        h1 { "Quadlets" }
        p.muted { "Managing " code { (quadlet_dir) } }
        table.units {
            thead { tr { th { "File" } th { "Kind" } th { "Service" } th { "Status" } th { "Actions" } } }
            // Kept present (with an empty-state row) even when there are no
            // units yet, so this stays a valid `sse-swap="units-changed"`
            // target if a unit is added later without a full page reload.
            tbody id="unit-rows" sse-swap="units-changed" hx-swap="innerHTML" {
                (unit_rows(units, csrf))
            }
        }
    };
    shell("Dashboard", body)
}

fn section_table(unit: &QuadletUnit) -> Markup {
    html! {
        @for section in &unit.sections {
            h3 { "[" (section.name) "]" }
            table.kv {
                @for (k, v) in &section.entries {
                    tr { td.key { (k) } td.value { (v) } }
                }
            }
        }
    }
}

pub fn unit_detail_page(unit: &QuadletUnit, status: &UnitStatus, csrf: &str) -> Markup {
    let service = unit.service_name();
    let body = html! {
        h1 { (unit.file_name) }
        p.muted { code { (unit.path.display().to_string()) } }
        p { (status_badge(&service, status)) }
        @if !unit.is_template() {
            (actions(unit, status, csrf))
        }
        p.links {
            a href={"/units/" (unit.file_name) "/edit"} { "Edit" }
            " · "
            a href={"/units/" (unit.file_name) "/logs"} { "Logs" }
            " · "
            (delete_form(&unit.file_name, csrf))
        }
        h2 { "Parsed contents" }
        (section_table(unit))
    };
    shell(&unit.file_name, body)
}

fn delete_form(file_name: &str, csrf: &str) -> Markup {
    html! {
        form style="display:inline"
            method="post"
            action={"/units/" (file_name) "/delete"}
            onsubmit="return confirm('Delete this quadlet file? This cannot be undone.')" {
            (csrf_input(csrf))
            button type="submit" class="btn btn-danger link" { "Delete" }
        }
    }
}

pub fn new_unit_page(csrf: &str, error: Option<&str>) -> Markup {
    let body = html! {
        h1 { "New quadlet" }
        @if let Some(msg) = error {
            div.error-banner { (msg) }
        }
        form method="post" action="/units" {
            (csrf_input(csrf))
            label { "File name" }
            input type="text" name="file_name" placeholder="myapp.container" required;
            p.muted { "Valid extensions: " (kind_names().join(", ")) }
            label { "Contents" }
            textarea name="contents" rows="16" placeholder="[Container]\nImage=docker.io/library/alpine\nExec=sleep infinity\n" required {}
            button type="submit" class="btn btn-primary" { "Create" }
        }
    };
    shell("New quadlet", body)
}

pub fn edit_unit_page(unit: &QuadletUnit, csrf: &str, error: Option<&str>) -> Markup {
    let body = html! {
        h1 { "Edit " (unit.file_name) }
        @if let Some(msg) = error {
            div.error-banner { (msg) }
        }
        form method="post" action={"/units/" (unit.file_name) "/edit"} {
            (csrf_input(csrf))
            textarea name="contents" rows="20" { (unit.raw) }
            button type="submit" class="btn btn-primary" { "Save" }
        }
    };
    shell(&format!("Edit {}", unit.file_name), body)
}

pub fn logs_page(unit: &QuadletUnit, initial: &str) -> Markup {
    let service = unit.service_name();
    let body = html! {
        h1 { "Logs: " (unit.file_name) }
        p.muted { (service) }
        pre id="log-output" { (initial) }
        script {
            // The stream route is keyed by the quadlet *file* name (like every
            // other /units/... route), not the systemd service name.
            (maud::PreEscaped(format!(
                r#"const out = document.getElementById("log-output");
                const es = new EventSource("/units/{file_name}/logs/stream");
                es.onmessage = (e) => {{
                    out.textContent += e.data + "\n";
                    out.scrollTop = out.scrollHeight;
                }};
                window.addEventListener("beforeunload", () => es.close());"#,
                file_name = unit.file_name,
            )))
        }
    };
    shell(&format!("Logs: {}", unit.file_name), body)
}

pub fn login_page(error: Option<&str>) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Sign in · sooth" }
                link rel="stylesheet" href="/static/style.css";
            }
            body.login-body {
                main.login-box {
                    h1 { "sooth" }
                    @if let Some(msg) = error {
                        div.error-banner { (msg) }
                    }
                    form method="post" action="/login" {
                        label { "Password" }
                        input type="password" name="password" required autofocus;
                        button type="submit" class="btn btn-primary" { "Sign in" }
                    }
                }
            }
        }
    }
}

/// A list of unit kinds, used by the "new unit" help text and validation
/// messages -- kept here so the UI's notion of "which kinds exist" and the
/// domain model's stay in one place conceptually (`UnitKind::all()`).
pub fn kind_names() -> Vec<&'static str> {
    UnitKind::all().iter().map(|k| k.extension()).collect()
}

pub fn error_page(status: StatusCode, message: &str, id: Uuid) -> Markup {
    let body = html! {
        h1 { (status.as_u16()) " " (status.canonical_reason().unwrap_or("Error")) }
        p { (message) }
        p.muted { "Error ID: " code { (id.to_string()) } }
    };
    shell("Error", body)
}

pub fn error_fragment(message: &str, id: Uuid) -> Markup {
    html! {
        div.error-banner {
            (message)
            span.muted { " (id: " (id.to_string()) ")" }
        }
    }
}
