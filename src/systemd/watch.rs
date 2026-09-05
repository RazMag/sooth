use futures_util::StreamExt;
use tracing::{debug, warn};
use zbus::fdo::DBusProxy;
use zbus::message::Type as MessageType;
use zbus::{zvariant::ObjectPath, Connection, MatchRule, MessageStream};

use crate::events::{DashboardEvent, EventSender};

use super::client::ManagerProxy;
use super::Client;

const UNIT_PATH_PREFIX: &str = "/org/freedesktop/systemd1/unit/";

/// Subscribes to `PropertiesChanged` signals for every systemd unit object
/// and, on each one, re-fetches that unit's full status and forwards it as a
/// `DashboardEvent::Status` -- so the dashboard's SSE stream can push live
/// updates to open browser tabs within about a second of a real state change,
/// without polling.
///
/// Re-fetching a full status rather than trying to reconstruct one from the
/// (possibly partial) `PropertiesChanged` payload keeps this correct even
/// when only one property changed -- at the cost of one extra D-Bus
/// round-trip per event, which is cheap relative to a human watching a
/// dashboard.
///
/// This opens its own dedicated D-Bus connection rather than reusing
/// `client`'s: reading raw messages off a connection via `MessageStream`
/// competes with zbus's own reply dispatcher on that same connection, and
/// can intercept the reply meant for an in-flight method call (e.g. a
/// `StartUnit` issued concurrently from an action handler), leaving that
/// call hanging forever. A separate connection for watching signals avoids
/// the race entirely; `client`'s connection stays free for method calls.
///
/// Returns the task's `JoinHandle` so the caller can `abort()` it on
/// shutdown -- this loop never ends on its own (the connection has no
/// reason to close), and an un-aborted infinite task left running blocks
/// `tokio::runtime::Runtime`'s `Drop` forever, which otherwise silently
/// turns "shutdown signal received" into a process that never actually exits.
pub async fn spawn(client: Client, events: EventSender) -> zbus::Result<tokio::task::JoinHandle<()>> {
    let connection = Connection::session().await?;
    let manager = ManagerProxy::new(&connection).await?;
    manager.subscribe().await?;

    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .path_namespace(ObjectPath::try_from("/org/freedesktop/systemd1/unit")?)?
        .build();

    DBusProxy::new(&connection).await?.add_match_rule(rule).await?;

    let mut stream = MessageStream::from(connection);
    let handle = tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = %e, "systemd watch: error reading D-Bus message");
                    continue;
                }
            };
            let Some(path) = msg.header().path().map(|p| p.as_str().to_string()) else {
                continue;
            };
            let Some(service) = unescape_unit_path(&path) else {
                continue;
            };
            match client.status(&service).await {
                Ok(status) => {
                    debug!(service, ?status, "unit status changed");
                    let _ = events.send(DashboardEvent::Status { service, status });
                }
                Err(e) => warn!(service, error = %e, "failed to refresh status after change notification"),
            }
        }
    });
    Ok(handle)
}

/// Reverses systemd's D-Bus object-path escaping (each byte outside
/// `[A-Za-z0-9]` is encoded as `_xx` hex) to recover the unit name from an
/// object path such as `/org/freedesktop/systemd1/unit/myapp_2eservice`.
fn unescape_unit_path(path: &str) -> Option<String> {
    let encoded = path.strip_prefix(UNIT_PATH_PREFIX)?;
    let mut out = String::with_capacity(encoded.len());
    let mut chars = encoded.chars();
    while let Some(c) = chars.next() {
        if c == '_' {
            let hi = chars.next()?;
            let lo = chars.next()?;
            let byte = u8::from_str_radix(&format!("{hi}{lo}"), 16).ok()?;
            out.push(byte as char);
        } else {
            out.push(c);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_simple_unit_path() {
        assert_eq!(
            unescape_unit_path("/org/freedesktop/systemd1/unit/myapp_2eservice"),
            Some("myapp.service".to_string())
        );
    }

    #[test]
    fn unescapes_at_and_dash() {
        assert_eq!(
            unescape_unit_path("/org/freedesktop/systemd1/unit/foo_40bar_2dbaz_2eservice"),
            Some("foo@bar-baz.service".to_string())
        );
    }

    #[test]
    fn rejects_unrelated_path() {
        assert_eq!(unescape_unit_path("/org/freedesktop/systemd1/job/1"), None);
    }
}
