use zbus::{Connection, proxy, zvariant::OwnedObjectPath};

use super::SystemdError;
use super::status::{self, UnitStatus};

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
    fn disable_unit_files(
        &self,
        files: &[&str],
        runtime: bool,
    ) -> zbus::Result<Vec<(String, String, String)>>;
    /// Required once at startup for `PropertiesChanged` signals on unit
    /// objects to actually be emitted to this connection.
    fn subscribe(&self) -> zbus::Result<()>;

    /// The user manager's environment block -- exactly what `systemctl --user
    /// show-environment` prints, as `KEY=VALUE` strings. This is the variable
    /// set the quadlet generator and the units it generates run with, so it's
    /// what a `${NAME}` reference inside a quadlet file resolves against. Not
    /// cached: it's mutated out of band (`set-environment`, `import-environment`,
    /// `environment.d`), never via a `PropertiesChanged` signal.
    #[zbus(property(emits_changed_signal = "false"))]
    fn environment(&self) -> zbus::Result<Vec<String>>;

    /// Add/replace `KEY=VALUE` assignments in the running manager's
    /// environment (the `systemctl --user set-environment` D-Bus call).
    /// Runtime-only -- persistence is `environment.d`, handled separately.
    fn set_environment(&self, assignments: &[&str]) -> zbus::Result<()>;

    /// Drop the named variables from the running manager's environment
    /// (`systemctl --user unset-environment`).
    fn unset_environment(&self, names: &[&str]) -> zbus::Result<()>;
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
        manager
            .subscribe()
            .await
            .map_err(|e| SystemdError::action_failed("(daemon)", "subscribe", e))?;
        Ok(Self {
            connection,
            manager,
        })
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
        self.manager
            .reload()
            .await
            .map_err(|e| SystemdError::action_failed("(daemon)", "reload", e))
    }

    pub async fn status(&self, unit: &str) -> Result<UnitStatus, SystemdError> {
        status::fetch(&self.connection, &self.manager, unit).await
    }

    /// The user manager's environment as `(name, value)` pairs sorted by
    /// name. A bare `NAME` with no `=` (systemd permits it) yields an empty
    /// value.
    pub async fn environment(&self) -> Result<Vec<(String, String)>, SystemdError> {
        let raw = self
            .manager
            .environment()
            .await
            .map_err(|e| SystemdError::action_failed("(daemon)", "show-environment", e))?;
        let mut pairs: Vec<(String, String)> = raw
            .iter()
            .map(|entry| match entry.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (entry.clone(), String::new()),
            })
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(pairs)
    }

    /// Applies `NAME=VALUE` assignments to the running user manager so they're
    /// usable immediately, without waiting for the next login to re-read
    /// `environment.d`.
    pub async fn set_environment(&self, assignments: &[String]) -> Result<(), SystemdError> {
        let refs: Vec<&str> = assignments.iter().map(String::as_str).collect();
        self.manager
            .set_environment(&refs)
            .await
            .map_err(|e| SystemdError::action_failed("(daemon)", "set-environment", e))
    }

    /// Removes the named variables from the running user manager's environment.
    pub async fn unset_environment(&self, names: &[String]) -> Result<(), SystemdError> {
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        self.manager
            .unset_environment(&refs)
            .await
            .map_err(|e| SystemdError::action_failed("(daemon)", "unset-environment", e))
    }
}
