use std::time::Duration;

use futures_util::StreamExt;
use zbus::{
    Connection, proxy,
    zvariant::{ObjectPath, OwnedObjectPath},
};

use super::SystemdError;
use super::status::{self, UnitStatus};

/// Thin proxy over `org.freedesktop.systemd1.Manager` on the session bus --
/// the interface systemd exposes for exactly this dashboard's needs
/// (start/stop/restart/reload/status), confirmed present via
/// `busctl --user introspect org.freedesktop.systemd1 /org/freedesktop/systemd1`.
///
/// Note there is no enable/disable here: podman's generated `.service` units
/// live under a systemd generator directory, which `EnableUnitFiles` rejects
/// ("transient or generated"). Autostart is toggled by patching the quadlet
/// file's `[Install]` section instead -- see `quadlet::install`.
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
    /// `systemctl --user reset-failed <name>`: a `failed` unit back to
    /// `inactive`.
    fn reset_failed_unit(&self, name: &str) -> zbus::Result<()>;
    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    /// Required once at startup for `PropertiesChanged` signals on unit
    /// objects to actually be emitted to this connection.
    fn subscribe(&self) -> zbus::Result<()>;

    /// A queued job finished -- for a stop job, once the unit has reached
    /// `inactive` or `failed`.
    #[zbus(signal)]
    fn job_removed(
        &self,
        id: u32,
        job: ObjectPath<'_>,
        unit: &str,
        result: &str,
    ) -> zbus::Result<()>;

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

    /// Queues a stop, and once it has finished clears a `failed` state the
    /// stop itself left behind (see [`clear_failed_stop`]). Returns as soon
    /// as the job is queued, like the other actions; the status watch shows
    /// the unit going down.
    pub async fn stop(&self, unit: &str) -> Result<(), SystemdError> {
        // Listen before queueing, so a job that finishes at once isn't missed.
        let removed = self
            .manager
            .receive_job_removed()
            .await
            .map_err(|e| SystemdError::action_failed(unit, "stop", e))?;
        let job = self
            .manager
            .stop_unit(unit, "replace")
            .await
            .map_err(|e| SystemdError::action_failed(unit, "stop", e))?;
        tokio::spawn(clear_failed_stop(
            self.clone(),
            unit.to_string(),
            job,
            removed,
        ));
        Ok(())
    }

    pub async fn restart(&self, unit: &str) -> Result<(), SystemdError> {
        self.manager
            .restart_unit(unit, "replace")
            .await
            .map(|_| ())
            .map_err(|e| SystemdError::action_failed(unit, "restart", e))
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

    pub async fn reset_failed(&self, unit: &str) -> Result<(), SystemdError> {
        self.manager
            .reset_failed_unit(unit)
            .await
            .map_err(|e| SystemdError::action_failed(unit, "reset-failed", e))
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

/// How long [`clear_failed_stop`] waits for a stop job before giving up on
/// it: well past systemd's default 90s `TimeoutStopSec=`, after which the
/// job ends one way or another.
const STOP_JOB_WAIT: Duration = Duration::from_secs(180);

/// Once the stop job `job` for `unit` has finished, resets the unit if it
/// ended up `failed`.
///
/// A unit sooth was asked to stop shouldn't show as failed for having
/// stopped. But a podman container regularly does: quadlet's `ExecStop=` is
/// `podman rm -f`, which SIGKILLs a container still running after its stop
/// timeout (one whose PID 1 ignores SIGTERM, or is just slow), the main
/// process then exits 137, and systemd records `Result=exit-code`. The
/// journal keeps the exit status; this only clears the state. A unit that
/// crashed on its own, without a stop from sooth, is left failed.
async fn clear_failed_stop(
    client: Client,
    unit: String,
    job: OwnedObjectPath,
    mut removed: JobRemovedStream,
) {
    let finished = async {
        while let Some(signal) = removed.next().await {
            if signal
                .args()
                .is_ok_and(|args| args.job.as_str() == job.as_str())
            {
                return true;
            }
        }
        false
    };
    if !matches!(
        tokio::time::timeout(STOP_JOB_WAIT, finished).await,
        Ok(true)
    ) {
        tracing::debug!(
            unit,
            "stop job not seen finishing; not checking for a failed state"
        );
        return;
    }
    match client.status(&unit).await {
        Ok(status) if status.is_failed() => match client.reset_failed(&unit).await {
            Ok(()) => tracing::info!(unit, "unit failed while stopping; reset to inactive"),
            Err(e) => {
                tracing::warn!(unit, error = %e, "reset of a unit that failed while stopping failed")
            }
        },
        Ok(_) => {}
        Err(e) => tracing::warn!(unit, error = %e, "status check after stop failed"),
    }
}
