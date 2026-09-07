use std::collections::HashMap;

use zbus::{Connection, fdo::PropertiesProxy, names::InterfaceName, zvariant::OwnedValue};

use super::SystemdError;
use super::client::ManagerProxy;

/// A unit's live status as reported by systemd over D-Bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitStatus {
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub description: String,
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
        }
    }

    pub fn is_active(&self) -> bool {
        self.active_state == "active"
    }

    pub fn is_failed(&self) -> bool {
        self.active_state == "failed"
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

    Ok(UnitStatus {
        load_state: get_str(&unit_props, "LoadState"),
        active_state: get_str(&unit_props, "ActiveState"),
        sub_state: get_str(&unit_props, "SubState"),
        description: get_str(&unit_props, "Description"),
    })
}

fn get_str(map: &HashMap<String, OwnedValue>, key: &str) -> String {
    map.get(key)
        .and_then(|v| String::try_from(v.clone()).ok())
        .unwrap_or_default()
}
