//! Reads a unit's journal by shelling out to `journalctl --user`. This is the
//! one deliberate exception to "D-Bus for everything" in this app: the
//! journal isn't usefully exposed over D-Bus for streaming, and binding
//! `sd-journal` over FFI isn't justified just to tail recent log lines.
//!
//! Safety note: `service` here must always be a systemd unit name derived by
//! this app (`QuadletUnit::service_name()`), never raw user input passed
//! straight through -- callers must not build these commands from anything
//! else, which forecloses argument injection.
//!
//! Both reads ask for `-o json` and parse each entry into a [`LogLine`], so
//! every line carries the systemd invocation ID of the run that produced it --
//! that's what lets the logs page tell one start of a unit from the previous.

use serde_json::Value;
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdout, Command};

use crate::systemd::SystemdError;

/// One journal entry, reduced to what the logs page renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// `__REALTIME_TIMESTAMP`: microseconds since the Unix epoch.
    pub realtime_us: i64,
    /// The run this entry belongs to. The unit's own output carries
    /// `_SYSTEMD_INVOCATION_ID`; the user manager's "Starting…/Failed…"
    /// messages about it carry `USER_INVOCATION_ID` instead.
    pub invocation: Option<String>,
    pub ident: String,
    pub pid: Option<String>,
    pub message: String,
}

/// Parses one line of `journalctl -o json` output. `None` for anything that
/// isn't an entry with a message (malformed JSON, `MESSAGE` null/missing).
pub fn parse_json_line(line: &str) -> Option<LogLine> {
    let entry: Value = serde_json::from_str(line).ok()?;
    let field = |name: &str| match entry.get(name)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    let message = match entry.get("MESSAGE")? {
        Value::String(s) => s.clone(),
        // journald encodes non-UTF-8 (or binary) fields as a byte array.
        Value::Array(bytes) => {
            let bytes: Vec<u8> = bytes
                .iter()
                .filter_map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => return None,
    };
    Some(LogLine {
        realtime_us: field("__REALTIME_TIMESTAMP")?.parse().ok()?,
        invocation: field("_SYSTEMD_INVOCATION_ID")
            .or_else(|| field("USER_INVOCATION_ID"))
            .or_else(|| field("INVOCATION_ID")),
        ident: field("SYSLOG_IDENTIFIER")
            .or_else(|| field("_COMM"))
            .unwrap_or_default(),
        pid: field("_PID"),
        message,
    })
}

/// Reads the most recent `lines` entries of a unit's journal for the initial
/// page load.
pub async fn tail_recent(service: &str, lines: u32) -> Result<Vec<LogLine>, SystemdError> {
    let output = Command::new("journalctl")
        .args([
            "--user",
            "-u",
            service,
            "-n",
            &lines.to_string(),
            "--no-pager",
            "-o",
            "json",
        ])
        .output()
        .await
        .map_err(|e| SystemdError::action_failed(service, "read logs", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SystemdError::action_failed(service, "read logs", stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_json_line)
        .collect())
}

/// Spawns `journalctl --user -f` for live tailing. The child is configured
/// to be killed when dropped, so the caller only needs to keep it alive for
/// as long as it wants the tail to keep running (e.g. for the lifetime of an
/// SSE connection). Each stdout line is one `-o json` entry for
/// [`parse_json_line`].
pub fn follow(service: &str) -> std::io::Result<(Child, BufReader<ChildStdout>)> {
    let mut child = Command::new("journalctl")
        .args([
            "--user", "-u", service, "-f", "-o", "json", "--since", "now",
        ])
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("child stdout was not piped (this is a bug)"))?;
    Ok((child, BufReader::new(stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unit_output() {
        let line = parse_json_line(
            r#"{"__REALTIME_TIMESTAMP":"1790000000123456","_SYSTEMD_INVOCATION_ID":"abc","SYSLOG_IDENTIFIER":"web","_PID":"42","MESSAGE":"hello"}"#,
        )
        .unwrap();
        assert_eq!(
            line,
            LogLine {
                realtime_us: 1_790_000_000_123_456,
                invocation: Some("abc".into()),
                ident: "web".into(),
                pid: Some("42".into()),
                message: "hello".into(),
            }
        );
    }

    #[test]
    fn manager_messages_use_user_invocation_id() {
        let line = parse_json_line(
            r#"{"__REALTIME_TIMESTAMP":"1","USER_INVOCATION_ID":"run2","_COMM":"systemd","MESSAGE":"Starting web.service..."}"#,
        )
        .unwrap();
        assert_eq!(line.invocation.as_deref(), Some("run2"));
        assert_eq!(line.ident, "systemd");
        assert_eq!(line.pid, None);
    }

    #[test]
    fn byte_array_message_is_decoded_lossily() {
        let line =
            parse_json_line(r#"{"__REALTIME_TIMESTAMP":"1","MESSAGE":[104,105,255]}"#).unwrap();
        assert_eq!(line.message, "hi\u{fffd}");
        assert_eq!(line.invocation, None);
    }

    #[test]
    fn rejects_non_entries() {
        assert_eq!(parse_json_line("not json"), None);
        assert_eq!(
            parse_json_line(r#"{"__REALTIME_TIMESTAMP":"1","MESSAGE":null}"#),
            None
        );
        assert_eq!(parse_json_line(r#"{"MESSAGE":"no timestamp"}"#), None);
    }
}
