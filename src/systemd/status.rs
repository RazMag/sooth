use std::collections::HashMap;

use zbus::{Connection, fdo::PropertiesProxy, names::InterfaceName, zvariant::OwnedValue};

use super::SystemdError;
use super::client::ManagerProxy;

/// The target a rootless `systemctl --user` login reaches; a unit wanted or
/// required by it starts on login. The rootless equivalent of
/// `multi-user.target` -- see `quadlet::install`'s `MANAGED_LINE`.
const LOGIN_TARGET: &str = "default.target";

/// A unit's live status as reported by systemd over D-Bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitStatus {
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub description: String,
    /// The targets that pull this unit in, from systemd's *computed*
    /// `WantedBy=` / `RequiredBy=` reverse dependencies -- i.e. the
    /// `[Install]` section *after* the quadlet generator has turned it into
    /// `<target>.wants/` symlinks. This is the real autostart wiring, unlike
    /// the raw file, which can name a target that doesn't exist or isn't in
    /// the login path.
    pub autostart_targets: Vec<String>,
    /// When the unit last entered `active`, in microseconds since the Unix
    /// epoch (systemd's `ActiveEnterTimestamp`); 0 when it never has.
    pub active_since_usec: u64,
    /// When the unit's active state last changed at all
    /// (`StateChangeTimestamp`); 0 when unknown.
    pub state_change_usec: u64,
    /// Why a failed service failed, already worded for display ("exit 1",
    /// "signal", "timeout"...). Only fetched for a failed unit.
    pub failure: Option<String>,
}

impl UnitStatus {
    /// The state to render when systemd has no loaded unit by this name yet
    /// (e.g. the quadlet file was just added and `Reload()` hasn't produced
    /// the `.service` unit, or it was never started) -- a legitimate,
    /// expected state, not an error.
    pub fn not_found() -> Self {
        Self {
            load_state: "not-found".into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            description: String::new(),
            autostart_targets: Vec::new(),
            active_since_usec: 0,
            state_change_usec: 0,
            failure: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active_state == "active"
    }

    pub fn is_failed(&self) -> bool {
        self.active_state == "failed"
    }

    /// True when this unit actually autostarts on a rootless login -- i.e.
    /// systemd computed `default.target` as one of its `WantedBy=` /
    /// `RequiredBy=` reverse deps. An `[Install]` section pointing at a
    /// missing or non-login target leaves this false, matching reality.
    pub fn is_autostart_enabled(&self) -> bool {
        self.autostart_targets.iter().any(|t| t == LOGIN_TARGET)
    }
}

pub(super) async fn fetch(
    connection: &Connection,
    manager: &ManagerProxy<'_>,
    unit: &str,
) -> Result<UnitStatus, SystemdError> {
    let path = match manager.get_unit(unit).await {
        Ok(p) => p,
        // Not loaded is a normal, transient state (unit never started, or the
        // generator hasn't produced it yet) -- not a hard error.
        Err(_) => return Ok(UnitStatus::not_found()),
    };

    let props = PropertiesProxy::builder(connection)
        .destination("org.freedesktop.systemd1")
        .map_err(|e| SystemdError::action_failed(unit, "status", e))?
        .path(path)
        .map_err(|e| SystemdError::action_failed(unit, "status", e))?
        .build()
        .await
        .map_err(|e| SystemdError::action_failed(unit, "status", e))?;

    const UNIT_IFACE: InterfaceName<'static> =
        InterfaceName::from_static_str_unchecked("org.freedesktop.systemd1.Unit");

    let unit_props = props
        .get_all(UNIT_IFACE)
        .await
        .map_err(|e| SystemdError::action_failed(unit, "status", e))?;

    // `WantedBy` / `RequiredBy` on the Unit interface are the *reverse* deps
    // systemd computed from every loaded `<target>.wants/` dir -- so this
    // reflects the symlink the quadlet generator wrote for a real `[Install]`
    // target and stays empty for one that names a bogus/unloaded target.
    let mut autostart_targets = get_strv(&unit_props, "WantedBy");
    autostart_targets.extend(get_strv(&unit_props, "RequiredBy"));

    let active_state = get_str(&unit_props, "ActiveState");

    // Only a failed unit needs the Service interface, for its exit status --
    // one extra round trip, never paid on the common path.
    let failure = if active_state == "failed" {
        const SERVICE_IFACE: InterfaceName<'static> =
            InterfaceName::from_static_str_unchecked("org.freedesktop.systemd1.Service");
        match props.get_all(SERVICE_IFACE).await {
            Ok(svc) => failure_text(&get_str(&svc, "Result"), get_i32(&svc, "ExecMainStatus")),
            Err(_) => None,
        }
    } else {
        None
    };

    Ok(UnitStatus {
        load_state: get_str(&unit_props, "LoadState"),
        active_state,
        sub_state: get_str(&unit_props, "SubState"),
        description: get_str(&unit_props, "Description"),
        autostart_targets,
        active_since_usec: get_u64(&unit_props, "ActiveEnterTimestamp"),
        state_change_usec: get_u64(&unit_props, "StateChangeTimestamp"),
        failure,
    })
}

/// A failed service's `Result=` and main process exit status, as a short
/// phrase for the status cell. `exit-code` is the common case and reads as
/// "exit 1"; other results ("signal", "timeout", "core-dump", ...) are shown
/// as systemd names them. `success`/empty gives nothing to show.
fn failure_text(result: &str, exit_status: Option<i32>) -> Option<String> {
    match result {
        "" | "success" => None,
        "exit-code" => Some(match exit_status {
            Some(code) => format!("exit {code}"),
            None => "exit code".to_string(),
        }),
        other => Some(other.replace('-', " ")),
    }
}

fn get_str(map: &HashMap<String, OwnedValue>, key: &str) -> String {
    map.get(key)
        .and_then(|v| String::try_from(v.clone()).ok())
        .unwrap_or_default()
}

fn get_u64(map: &HashMap<String, OwnedValue>, key: &str) -> u64 {
    map.get(key)
        .and_then(|v| u64::try_from(v.clone()).ok())
        .unwrap_or_default()
}

fn get_i32(map: &HashMap<String, OwnedValue>, key: &str) -> Option<i32> {
    map.get(key).and_then(|v| i32::try_from(v.clone()).ok())
}

fn get_strv(map: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(|v| Vec::<String>::try_from(v.clone()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_targets(targets: &[&str]) -> UnitStatus {
        UnitStatus {
            autostart_targets: targets.iter().map(|s| s.to_string()).collect(),
            ..UnitStatus::not_found()
        }
    }

    #[test]
    fn failure_text_words_the_common_results() {
        assert_eq!(
            failure_text("exit-code", Some(1)).as_deref(),
            Some("exit 1")
        );
        assert_eq!(
            failure_text("exit-code", None).as_deref(),
            Some("exit code")
        );
        assert_eq!(failure_text("signal", Some(9)).as_deref(), Some("signal"));
        assert_eq!(
            failure_text("core-dump", None).as_deref(),
            Some("core dump")
        );
        assert_eq!(failure_text("success", Some(0)), None);
        assert_eq!(failure_text("", None), None);
    }

    #[test]
    fn autostart_needs_the_login_target_specifically() {
        assert!(with_targets(&["default.target"]).is_autostart_enabled());
        assert!(with_targets(&["some.target", "default.target"]).is_autostart_enabled());
        // An [Install] section pointing at a target that doesn't exist or
        // isn't in the login path: systemd computes no default.target dep.
        assert!(!with_targets(&["bogus.target"]).is_autostart_enabled());
        assert!(!with_targets(&["multi-user.target"]).is_autostart_enabled());
        assert!(!with_targets(&[]).is_autostart_enabled());
        assert!(!UnitStatus::not_found().is_autostart_enabled());
    }
}
