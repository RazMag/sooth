use zbus::{proxy, zvariant::OwnedObjectPath, Connection};

use super::status::{self, UnitStatus};
use super::SystemdError;

/// Thin proxy over `org.freedesktop.systemd1.Manager` on the session bus --
/// the interface systemd exposes for exactly this dashboard's needs
/// (start/stop/restart/enable/disable/reload/status), confirmed present via
/// `busctl --user introspect org.freedesktop.systemd1 /org/freedesktop/systemd1`.
#[proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
pub trait Manager {
    fn start_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn stop_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn restart_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;
    fn reload(&self) -> zbus::Result<()>;
    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    fn enable_unit_files(
        &self,
        files: &[&str],
        runtime: bool,
        force: bool,
    ) -> zbus::Result<(bool, Vec<(String, String, String)>)>;
    fn disable_unit_files(&self, files: &[&str], runtime: bool) -> zbus::Result<Vec<(String, String, String)>>;
    /// Required once at startup for `PropertiesChanged` signals on unit
    /// objects to actually be emitted to this connection.
    fn subscribe(&self) -> zbus::Result<()>;
}

/// A connected client to the user session's systemd D-Bus manager. Cheap to
/// clone: `Connection` and the generated proxy are both internally
/// reference-counted by zbus.
#[derive(Clone)]
pub struct Client {
    connection: Connection,
    manager: ManagerProxy<'static>,
}

impl Client {
    pub async fn connect() -> Result<Self, SystemdError> {
        let connection = Connection::session().await?;
        let manager = ManagerProxy::new(&connection).await?;
        manager.subscribe().await.map_err(|e| SystemdError::action_failed("(daemon)", "subscribe", e))?;
        Ok(Self { connection, manager })
    }

    pub async fn start(&self, unit: &str) -> Result<(), SystemdError> {
        self.manager
            .start_unit(unit, "replace")
            .await
            .map(|_| ())
            .map_err(|e| SystemdError::action_failed(unit, "start", e))
    }

    pub async fn stop(&self, unit: &str) -> Result<(), SystemdError> {
        self.manager
            .stop_unit(unit, "replace")
            .await
            .map(|_| ())
            .map_err(|e| SystemdError::action_failed(unit, "stop", e))
    }

    pub async fn restart(&self, unit: &str) -> Result<(), SystemdError> {
        self.manager
            .restart_unit(unit, "replace")
            .await
            .map(|_| ())
            .map_err(|e| SystemdError::action_failed(unit, "restart", e))
    }

    pub async fn enable(&self, unit: &str) -> Result<(), SystemdError> {
        self.manager
            .enable_unit_files(&[unit], false, true)
            .await
            .map(|_| ())
            .map_err(|e| SystemdError::action_failed(unit, "enable", e))
    }

    pub async fn disable(&self, unit: &str) -> Result<(), SystemdError> {
        self.manager
            .disable_unit_files(&[unit], false)
            .await
            .map(|_| ())
            .map_err(|e| SystemdError::action_failed(unit, "disable", e))
    }

    /// Re-runs the quadlet generator (systemd's `daemon-reload` equivalent),
    /// needed after any quadlet file create/edit/delete before the resulting
    /// `.service` unit can be queried or started.
    pub async fn reload(&self) -> Result<(), SystemdError> {
        self.manager.reload().await.map_err(|e| SystemdError::action_failed("(daemon)", "reload", e))
    }

    pub async fn status(&self, unit: &str) -> Result<UnitStatus, SystemdError> {
        status::fetch(&self.connection, &self.manager, unit).await
    }
}
