//! One logs page template, reused by every section (the log stream is
//! kind-agnostic -- it's just `journalctl` against the unit's service name).
//! The live tail is wired up by `frontend/logs.js` off the `data-log-stream`
//! attribute; there is no page-specific inline script.
//!
//! Lines are grouped into *runs* by systemd invocation ID. Every line not from
//! the latest run is marked `.log-old`, and a `.log-run-divider` separates
//! runs; the "Latest run only" toggle (on by default) just hides the old ones
//! via CSS. `logs.js` applies the same marking to lines arriving live.

use maud::{Markup, html};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{NavItem, back_link, page_header, shell};
use crate::health::Health;
use crate::journal::LogLine;
use crate::quadlet::QuadletUnit;
use crate::web::core;

pub fn logs_page(unit: &QuadletUnit, lines: &[LogLine], health: Health) -> Markup {
    let service = unit.service_name();
    let stream_url = format!("{}/logs/stream", core::unit_url(unit));
    let latest = lines.iter().rev().find_map(|l| l.invocation.as_deref());
    let actions = html! {
        label.checkbox-line {
            input type="checkbox" data-log-latest checked;
            span { "Latest run only" }
        }
    };
    let body = html! {
        (back_link(&core::unit_url(unit), &unit.file_name))
        (page_header(&format!("Logs: {}", unit.file_name), actions))
        p.page-meta { code { (service) } }
        div #log-output .latest-only data-log-stream data-stream-url=(stream_url)
            data-latest-inv=[latest] {
            @if lines.is_empty() {
                p.empty data-log-empty { "No log entries yet." }
            }
            @for row in rows(lines, latest) { (row) }
        }
    };
    shell(
        &format!("Logs: {}", unit.file_name),
        NavItem::for_kind(unit.kind),
        Some(health),
        body,
    )
}

/// The lines plus a divider wherever the run changes. A line with no
/// invocation ID stays with the run before it (it never starts a new one).
fn rows(lines: &[LogLine], latest: Option<&str>) -> Vec<Markup> {
    let mut rows = Vec::with_capacity(lines.len());
    let mut current: Option<&str> = None;
    for line in lines {
        if let Some(inv) = line.invocation.as_deref() {
            if current.is_some_and(|c| c != inv) {
                rows.push(run_divider(line, Some(inv) != latest));
            }
            current = Some(inv);
        }
        rows.push(log_line(line, current.is_some() && current != latest));
    }
    rows
}

/// One journal entry. Also the payload of each `.../logs/stream` SSE message,
/// so live lines render exactly like the initial ones.
pub fn log_line(line: &LogLine, old: bool) -> Markup {
    let ts = timestamp(line.realtime_us);
    html! {
        div.log-line.log-old[old] data-inv=[line.invocation.as_deref()] {
            time datetime=(ts) { (ts) }
            " "
            span.log-ident {
                (line.ident)
                @if let Some(pid) = &line.pid { "[" (pid) "]" }
                ":"
            }
            " " (line.message)
        }
    }
}

fn run_divider(first: &LogLine, old: bool) -> Markup {
    let ts = timestamp(first.realtime_us);
    html! {
        div.log-run-divider.log-old[old] {
            span { "New run · " time datetime=(ts) { (ts) } }
        }
    }
}

/// RFC 3339 in UTC; `logs.js` rewrites the text into the viewer's local time.
fn timestamp(realtime_us: i64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(realtime_us) * 1000)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(inv: Option<&str>, message: &str) -> LogLine {
        LogLine {
            realtime_us: 1_790_000_000_000_000,
            invocation: inv.map(str::to_owned),
            ident: "web".into(),
            pid: Some("7".into()),
            message: message.into(),
        }
    }

    fn unit() -> QuadletUnit {
        QuadletUnit {
            file_name: "web.container".into(),
            group: String::new(),
            path: "web.container".into(),
            kind: crate::quadlet::UnitKind::Container,
            sections: Vec::new(),
            raw: String::new(),
        }
    }

    fn render(lines: &[LogLine]) -> String {
        let html = html! {
            @for l in lines { (log_line(l, false)) }
        };
        html.into_string()
    }

    #[test]
    fn log_line_escapes_message_and_carries_invocation() {
        let html = render(&[line(Some("abc"), "<b>hi</b>")]);
        assert!(html.contains(r#"data-inv="abc""#));
        assert!(html.contains("&lt;b&gt;hi&lt;/b&gt;"));
        assert!(html.contains("web[7]:"));
        assert!(html.contains("2026-09-21T"));
    }

    #[test]
    fn marks_old_runs_and_inserts_dividers() {
        let unit = unit();
        let lines = [
            line(Some("run1"), "old output"),
            line(None, "old, no inv"),
            line(Some("run2"), "new output"),
            line(None, "new, no inv"),
        ];
        let html = logs_page(&unit, &lines, Health::default()).into_string();
        assert!(html.contains(r#"data-latest-inv="run2""#));
        assert_eq!(html.matches("log-run-divider").count(), 1);
        assert!(
            html.contains(r#"class="log-run-divider""#),
            "latest run's divider stays visible"
        );
        assert_eq!(html.matches(r#"class="log-line log-old""#).count(), 2);
        assert_eq!(html.matches(r#"class="log-line""#).count(), 2);
        assert!(!html.contains("data-log-empty"));
    }

    #[test]
    fn empty_tail_shows_placeholder() {
        let unit = unit();
        let html = logs_page(&unit, &[], Health::default()).into_string();
        assert!(html.contains("data-log-empty"));
        assert!(!html.contains("data-latest-inv"));
    }
}
