//! Keeping a git-synced group's workloads in step with the checkout: which
//! units an update removes, changes or adds ([`UnitChanges`], pure, from a
//! `git diff` -- or from the whole tree on a fresh clone), and
//! stopping/restarting/starting them around the file change ([`UnitSync`]). The systemd side goes through [`UnitControl`] so tests can
//! drive the whole sequence against a recording fake instead of a session
//! bus.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tracing::{info, warn};

use crate::quadlet::UnitKind;
use crate::quadlet::naming;
use crate::systemd::{self, SystemdError, UnitStatus};

use super::git;

/// The five systemd calls a sync makes. Implemented by [`systemd::Client`];
/// object-safe (boxed futures) so `GitSyncManager` can hold one without
/// becoming generic.
pub trait UnitControl: Send + Sync {
    fn status<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<UnitStatus, SystemdError>>;
    fn start<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<(), SystemdError>>;
    fn stop<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<(), SystemdError>>;
    fn restart<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<(), SystemdError>>;
    fn reload(&self) -> BoxFuture<'_, Result<(), SystemdError>>;
}

impl UnitControl for systemd::Client {
    fn status<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<UnitStatus, SystemdError>> {
        Box::pin(systemd::Client::status(self, service))
    }
    fn start<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<(), SystemdError>> {
        Box::pin(systemd::Client::start(self, service))
    }
    fn stop<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<(), SystemdError>> {
        Box::pin(systemd::Client::stop(self, service))
    }
    fn restart<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<(), SystemdError>> {
        Box::pin(systemd::Client::restart(self, service))
    }
    fn reload(&self) -> BoxFuture<'_, Result<(), SystemdError>> {
        Box::pin(systemd::Client::reload(self))
    }
}

/// A long-running workload -- container, pod, kube unit -- the kinds a
/// sync stops, restarts or starts. Volumes, networks, images and builds are left
/// alone: their oneshot services don't reapply a changed definition to the
/// resource they already created, and stopping one never removes it (nor
/// should it -- a volume holds data). Templates have no service of their own.
fn is_workload(file_name: &str) -> bool {
    matches!(
        naming::kind_of(file_name),
        Some(UnitKind::Container | UnitKind::Pod | UnitKind::Kube)
    ) && !naming::is_template(file_name)
}

/// What moving a sync's checkout means for its workloads, worked out from
/// the diff *before* the move, by quadlet file name.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct UnitChanges {
    /// Deleted from the repo: stop them while systemd still has their
    /// generated definition loaded.
    removed: Vec<String>,
    /// Still there with their quadlet or a `<file>.d/` drop-in changed:
    /// restart them once the regenerated definition is loaded.
    changed: Vec<String>,
    /// New to the group: start the ones that would be up after a reboot
    /// anyway (see [`UnitSync::reload_and_apply`]).
    added: Vec<String>,
}

impl UnitChanges {
    fn from_diff(diff: &[git::FileChange]) -> Self {
        // file name -> blob, so a file only moved between directories (a
        // delete plus an add of the same content) counts as none of them.
        let mut deleted = BTreeMap::new();
        let mut added = BTreeMap::new();
        let mut modified = BTreeSet::new();
        let mut drop_ins = BTreeSet::new();
        for change in diff {
            let mut parts: Vec<&str> = change.path.split('/').collect();
            let Some(file_name) = parts.pop() else {
                continue;
            };
            // `discovery` skips dot-paths, so sooth doesn't treat these as
            // units anywhere else either.
            if parts.iter().any(|p| p.starts_with('.')) || file_name.starts_with('.') {
                continue;
            }
            // `web.container.d/10-x.conf` -> `web.container`.
            if let Some(unit) = parts.iter().rev().find_map(|p| p.strip_suffix(".d")) {
                if is_workload(unit) {
                    drop_ins.insert(unit);
                }
                continue;
            }
            if !is_workload(file_name) {
                continue;
            }
            match change.kind {
                git::ChangeKind::Deleted => {
                    deleted.insert(file_name, change.blob.as_str());
                }
                git::ChangeKind::Added => {
                    added.insert(file_name, change.blob.as_str());
                }
                git::ChangeKind::Modified => {
                    modified.insert(file_name);
                }
            }
        }

        let removed: Vec<String> = deleted
            .keys()
            .filter(|f| !added.contains_key(*f) && !modified.contains(*f))
            .map(|f| f.to_string())
            .collect();
        // Added under a name that was also deleted: moved, and restarted
        // only if its content changed on the way.
        let (moved, new): (BTreeMap<_, _>, BTreeMap<_, _>) = added
            .into_iter()
            .partition(|(f, _)| deleted.contains_key(f));
        let mut changed: BTreeSet<&str> = moved
            .into_iter()
            .filter(|(f, blob)| deleted.get(f) != Some(blob))
            .map(|(f, _)| f)
            .collect();
        changed.extend(modified);
        // A drop-in edit restarts its unit -- unless that unit is itself new
        // (it just starts, drop-in and all) or gone.
        changed.extend(
            drop_ins
                .into_iter()
                .filter(|f| !new.contains_key(f) && !removed.iter().any(|r| r == f)),
        );
        Self {
            removed,
            changed: changed.into_iter().map(str::to_string).collect(),
            added: new.into_keys().map(str::to_string).collect(),
        }
    }
}

/// `files` with pods first: (re)starting a pod brings its members along
/// (they're `BindsTo=` it, and it `Wants=` them), so going pods-first lets
/// a member changed or added in the same update be left to its pod rather
/// than restarted or started a second time.
fn pods_first(files: &[String]) -> impl Iterator<Item = &String> {
    let is_pod = |f: &&String| naming::kind_of(f) == Some(UnitKind::Pod);
    files
        .iter()
        .filter(is_pod)
        .chain(files.iter().filter(move |f| !is_pod(f)))
}

/// Keeps a group's workloads in step with a sync that moves its checkout
/// from one commit to another: `plan` before the move, then `stop_removed`
/// (still before it), then `reload_and_apply` after it. A fresh clone is
/// `plan_clone` + `reload_and_apply`: every unit in it counts as added.
/// Best-effort throughout: a failure is logged and never fails the sync
/// itself.
pub(super) struct UnitSync<'a> {
    systemd: Option<&'a dyn UnitControl>,
    group: &'a str,
    changes: UnitChanges,
}

impl<'a> UnitSync<'a> {
    /// Diffs `from`..`to` in the checkout at `target`. Plans nothing without
    /// a [`UnitControl`] (tests that don't care about units) or when the diff
    /// fails.
    pub(super) async fn plan(
        systemd: Option<&'a dyn UnitControl>,
        group: &'a str,
        target: &Path,
        from: &str,
        to: &str,
    ) -> Self {
        let diff = match systemd {
            None => Ok(Vec::new()),
            Some(_) => git::changed_files(target, from, to).await,
        };
        Self::from_files(systemd, group, diff)
    }

    /// Plans a checkout that was just cloned at `target`: every workload in
    /// it is new.
    pub(super) async fn plan_clone(
        systemd: Option<&'a dyn UnitControl>,
        group: &'a str,
        target: &Path,
    ) -> Self {
        let files = match systemd {
            None => Ok(Vec::new()),
            Some(_) => git::tracked_files(target).await,
        };
        Self::from_files(systemd, group, files)
    }

    fn from_files(
        systemd: Option<&'a dyn UnitControl>,
        group: &'a str,
        files: Result<Vec<git::FileChange>, super::GitSyncError>,
    ) -> Self {
        let changes = match files {
            Ok(files) => UnitChanges::from_diff(&files),
            Err(e) => {
                warn!(group, error = %e, "git-sync could not list changed files; \
                      not stopping, restarting or starting units");
                UnitChanges::default()
            }
        };
        Self {
            systemd,
            group,
            changes,
        }
    }

    /// Stops every running workload the move deletes, and waits for each to
    /// finish stopping. Must run *before* the files change: once the
    /// fs-watch task's reload sees the quadlet gone, systemd only knows a
    /// "not-found" unit with none of podman's stop/cleanup commands, and
    /// stopping it would just kill its processes and strand the container.
    pub(super) async fn stop_removed(&self) {
        let Some(systemd) = self.systemd else {
            return;
        };
        let group = self.group;
        for file_name in &self.changes.removed {
            let service = naming::service_name(file_name);
            match systemd.status(&service).await {
                Ok(st) if is_running(&st) => {}
                Ok(_) => continue,
                Err(e) => {
                    warn!(group, unit = %service, error = %e, "git-sync status check failed");
                    continue;
                }
            }
            if let Err(e) = systemd.stop(&service).await {
                warn!(group, unit = %service, error = %e, "stop of a unit removed by git-sync failed");
                continue;
            }
            if wait_until_stopped(systemd, &service, STOP_WAIT).await {
                info!(group, unit = %service, "stopped: removed by git-sync");
            } else {
                warn!(group, unit = %service, "unit removed by git-sync is still stopping; \
                      continuing anyway");
            }
        }
    }

    /// Reloads systemd, then restarts the changed workloads and starts the
    /// added ones. Must run *after* the files change.
    ///
    /// The reload comes first, from here, so each restart/start runs the
    /// regenerated unit. The fs-watch task in `main.rs` reloads for the same
    /// writes a moment later anyway, but it's debounced and unordered
    /// relative to this -- restarting before *some* reload has run would
    /// just bring the old definition back up. Nothing to restart or start,
    /// no reload.
    pub(super) async fn reload_and_apply(&self) {
        let Some(systemd) = self.systemd else {
            return;
        };
        if self.changes.changed.is_empty() && self.changes.added.is_empty() {
            return;
        }
        if let Err(e) = systemd.reload().await {
            warn!(group = self.group, error = %e, "git-sync reload failed; \
                  not restarting or starting units");
            return;
        }
        let mut brought_up = BTreeSet::new();
        self.restart_changed(systemd, &mut brought_up).await;
        self.start_added(systemd, &mut brought_up).await;
    }

    /// Restarts every changed workload that is running *or failed* -- a
    /// failed one may well be what the update fixes; a deliberately stopped
    /// one is left stopped. A member of a pod restarted here is skipped: it
    /// came back with the pod. Records each restarted service in
    /// `brought_up`.
    async fn restart_changed(&self, systemd: &dyn UnitControl, brought_up: &mut BTreeSet<String>) {
        let group = self.group;
        for file_name in pods_first(&self.changes.changed) {
            let service = naming::service_name(file_name);
            let st = match systemd.status(&service).await {
                Ok(st) if st.is_active() || st.is_failed() => st,
                Ok(_) => continue,
                Err(e) => {
                    warn!(group, unit = %service, error = %e, "git-sync status check failed");
                    continue;
                }
            };
            if let Some(pod) = wanted_by_any(&st, brought_up) {
                info!(group, unit = %service, pod, "restarted with its pod after git-sync update");
                continue;
            }
            match systemd.restart(&service).await {
                Ok(()) => {
                    info!(group, unit = %service, "restarted after git-sync update");
                    brought_up.insert(service);
                }
                Err(e) => {
                    warn!(group, unit = %service, error = %e, "restart after git-sync update failed")
                }
            }
        }
    }

    /// Starts every added workload that would be up after a reboot anyway:
    /// one whose `[Install]` section enables it (`WantedBy=default.target`),
    /// or a pod member whose pod is running (the pod `Wants=` its members,
    /// so it would have pulled this one in had it been there when the pod
    /// started). Anything else is only placed on disk -- the repo decides
    /// what runs. A member of a pod (re)started here is skipped: the pod
    /// pulls it in itself. Records each started service in `brought_up`.
    async fn start_added(&self, systemd: &dyn UnitControl, brought_up: &mut BTreeSet<String>) {
        let group = self.group;
        for file_name in pods_first(&self.changes.added) {
            let service = naming::service_name(file_name);
            let st = match systemd.status(&service).await {
                Ok(st) if !is_running(&st) => st,
                Ok(_) => continue,
                Err(e) => {
                    warn!(group, unit = %service, error = %e, "git-sync status check failed");
                    continue;
                }
            };
            if let Some(pod) = wanted_by_any(&st, brought_up) {
                info!(group, unit = %service, pod, "started with its pod after git-sync update");
                continue;
            }
            if !st.is_autostart_enabled() && !wanted_by_running(systemd, &st).await {
                continue;
            }
            match systemd.start(&service).await {
                Ok(()) => {
                    info!(group, unit = %service, "started: added by git-sync");
                    brought_up.insert(service);
                }
                Err(e) => {
                    warn!(group, unit = %service, error = %e, "start of a unit added by git-sync failed")
                }
            }
        }
    }
}

/// The first unit in `services` that pulls `status`'s unit in -- for a pod
/// member, its pod. (`autostart_targets` is systemd's computed `WantedBy=` /
/// `RequiredBy=`, which names the pod's service as well as any target.)
fn wanted_by_any<'s>(status: &UnitStatus, services: &'s BTreeSet<String>) -> Option<&'s str> {
    status
        .autostart_targets
        .iter()
        .find_map(|t| services.get(t))
        .map(String::as_str)
}

/// Whether any non-target unit that pulls `status`'s unit in -- a pod
/// member's pod -- is up.
async fn wanted_by_running(systemd: &dyn UnitControl, status: &UnitStatus) -> bool {
    for wanter in &status.autostart_targets {
        if wanter.ends_with(".target") {
            continue;
        }
        if let Ok(st) = systemd.status(wanter).await
            && is_running(&st)
        {
            return true;
        }
    }
    false
}

/// Up, or on its way up -- what a removed unit gets stopped from.
fn is_running(status: &UnitStatus) -> bool {
    matches!(
        status.active_state.as_str(),
        "active" | "activating" | "reloading"
    )
}

/// systemd's own default `TimeoutStopSec=` -- a unit still stopping after
/// this is stuck, not slow.
const STOP_WAIT: Duration = Duration::from_secs(90);

/// How often `wait_until_stopped` re-checks.
const STOP_POLL: Duration = Duration::from_millis(250);

/// Polls `service` until it has fully stopped (neither running nor
/// `deactivating`). `StopUnit` only queues the job; this is what lets the
/// caller change the files afterwards without racing the stop's own
/// cleanup. False on timeout or a status error.
async fn wait_until_stopped(systemd: &dyn UnitControl, service: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match systemd.status(service).await {
            Ok(st) if !is_running(&st) && st.active_state != "deactivating" => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(STOP_POLL).await;
    }
}

#[cfg(test)]
pub(super) mod fake {
    //! A recording [`UnitControl`] for tests, here and in `manager`.

    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;

    /// Holds each service's `ActiveState` (unknown = not loaded) and what
    /// pulls it in (`wanted_by`: a target, or a pod's service), and logs
    /// every mutating call in order. `stop` flips a unit to `inactive`
    /// unless `stuck_stopping`, and records whether `probe` (a quadlet path)
    /// still existed at that moment.
    #[derive(Default)]
    pub struct FakeUnits {
        pub states: Mutex<HashMap<String, String>>,
        pub wanted_by: HashMap<String, Vec<String>>,
        pub log: Mutex<Vec<String>>,
        pub probe: Option<PathBuf>,
        pub stuck_stopping: bool,
        pub fail_reload: bool,
    }

    impl FakeUnits {
        pub fn with_states(states: &[(&str, &str)]) -> Self {
            Self {
                states: Mutex::new(
                    states
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                ),
                ..Self::default()
            }
        }

        /// Sets what pulls each service in, e.g. `("web.service",
        /// "app-pod.service")` or `("app-pod.service", "default.target")`.
        pub fn wanted(mut self, pairs: &[(&str, &str)]) -> Self {
            for (service, by) in pairs {
                self.wanted_by
                    .entry(service.to_string())
                    .or_default()
                    .push(by.to_string());
            }
            self
        }

        pub fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        fn set(&self, service: &str, state: &str) {
            self.states
                .lock()
                .unwrap()
                .insert(service.to_string(), state.to_string());
        }
    }

    impl UnitControl for FakeUnits {
        fn status<'a>(
            &'a self,
            service: &'a str,
        ) -> BoxFuture<'a, Result<UnitStatus, SystemdError>> {
            let state = self.states.lock().unwrap().get(service).cloned();
            let autostart_targets = self.wanted_by.get(service).cloned().unwrap_or_default();
            Box::pin(async move {
                Ok(match state {
                    Some(active_state) => UnitStatus {
                        load_state: "loaded".into(),
                        active_state,
                        autostart_targets,
                        ..UnitStatus::not_found()
                    },
                    None => UnitStatus::not_found(),
                })
            })
        }

        fn start<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<(), SystemdError>> {
            self.log.lock().unwrap().push(format!("start {service}"));
            self.set(service, "active");
            Box::pin(async { Ok(()) })
        }

        fn stop<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<(), SystemdError>> {
            let file = self.probe.as_ref().map(|p| p.exists());
            let entry = match file {
                Some(present) => format!("stop {service} (file present: {present})"),
                None => format!("stop {service}"),
            };
            self.log.lock().unwrap().push(entry);
            self.set(
                service,
                if self.stuck_stopping {
                    "deactivating"
                } else {
                    "inactive"
                },
            );
            Box::pin(async { Ok(()) })
        }

        fn restart<'a>(&'a self, service: &'a str) -> BoxFuture<'a, Result<(), SystemdError>> {
            self.log.lock().unwrap().push(format!("restart {service}"));
            self.set(service, "active");
            Box::pin(async { Ok(()) })
        }

        fn reload(&self) -> BoxFuture<'_, Result<(), SystemdError>> {
            self.log.lock().unwrap().push("reload".to_string());
            let fail = self.fail_reload;
            Box::pin(async move {
                if fail {
                    Err(SystemdError::action_failed("(daemon)", "reload", "boom"))
                } else {
                    Ok(())
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeUnits;
    use super::*;
    use git::ChangeKind::{Added, Deleted, Modified};

    fn change(path: &str, kind: git::ChangeKind, blob: &str) -> git::FileChange {
        git::FileChange {
            path: path.to_string(),
            kind,
            blob: blob.to_string(),
        }
    }

    #[test]
    fn unit_changes_restart_edited_workloads_and_their_drop_ins() {
        let diff: Vec<_> = [
            "web.container",
            "media/db.container",
            "app.pod",
            "stack.kube",
            "api.container.d/10-env.conf",
            "data.volume",
            "net.network",
            "base.image",
            "img.build",
            "worker@.container",
            "worker@one.container",
            ".github/ci.container",
            "README.md",
            "data.volume.d/x.conf",
        ]
        .iter()
        .map(|p| change(p, Modified, "b"))
        .collect();
        let changes = UnitChanges::from_diff(&diff);
        assert!(changes.removed.is_empty());
        assert!(changes.added.is_empty());
        assert_eq!(
            changes.changed,
            [
                "api.container",
                "app.pod",
                "db.container",
                "stack.kube",
                "web.container",
                "worker@one.container",
            ]
        );
    }

    #[test]
    fn unit_changes_stop_deleted_workloads_only() {
        let diff = [
            change("old.container", Deleted, "a"),
            change("old.container.d/10-env.conf", Deleted, "a"),
            change("gone.pod", Deleted, "a"),
            change("data.volume", Deleted, "a"),
            change("worker@.container", Deleted, "a"),
            // A deleted drop-in of a kept unit restarts it.
            change("web.container.d/10-env.conf", Deleted, "a"),
        ];
        let changes = UnitChanges::from_diff(&diff);
        assert_eq!(changes.removed, ["gone.pod", "old.container"]);
        assert_eq!(changes.changed, ["web.container"]);
        assert!(changes.added.is_empty());
    }

    #[test]
    fn unit_changes_ignore_a_pure_move_but_restart_an_edited_one() {
        let diff = [
            change("web.container", Deleted, "same"),
            change("apps/web.container", Added, "same"),
            change("db.container", Deleted, "old"),
            change("apps/db.container", Added, "new"),
        ];
        let changes = UnitChanges::from_diff(&diff);
        assert!(changes.removed.is_empty());
        assert!(changes.added.is_empty());
        assert_eq!(changes.changed, ["db.container"]);
    }

    #[test]
    fn unit_changes_add_new_workloads_with_their_drop_ins() {
        let diff = [
            change("new.container", Added, "a"),
            // The new unit's own drop-in doesn't make it "changed" too.
            change("new.container.d/10-env.conf", Added, "a"),
            change("apps/app.pod", Added, "a"),
            change("data.volume", Added, "a"),
            change("worker@.container", Added, "a"),
        ];
        let changes = UnitChanges::from_diff(&diff);
        assert!(changes.removed.is_empty());
        assert!(changes.changed.is_empty());
        assert_eq!(changes.added, ["app.pod", "new.container"]);
    }

    #[test]
    fn pods_first_keeps_order_otherwise() {
        let files: Vec<String> = ["a.container", "b.pod", "c.kube", "d.pod"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let order: Vec<&str> = pods_first(&files).map(String::as_str).collect();
        assert_eq!(order, ["b.pod", "d.pod", "a.container", "c.kube"]);
    }

    fn sync<'a>(
        units: &'a FakeUnits,
        removed: &[&str],
        changed: &[&str],
        added: &[&str],
    ) -> UnitSync<'a> {
        let owned = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        UnitSync {
            systemd: Some(units),
            group: "g",
            changes: UnitChanges {
                removed: owned(removed),
                changed: owned(changed),
                added: owned(added),
            },
        }
    }

    #[tokio::test]
    async fn stop_removed_stops_only_running_units() {
        let units = FakeUnits::with_states(&[
            ("up.service", "active"),
            ("starting.service", "activating"),
            ("down.service", "inactive"),
            ("broken.service", "failed"),
        ]);
        sync(
            &units,
            &[
                "broken.container",
                "down.container",
                "never.container",
                "starting.container",
                "up.container",
            ],
            &[],
            &[],
        )
        .stop_removed()
        .await;
        assert_eq!(units.log(), ["stop starting.service", "stop up.service"]);
    }

    #[tokio::test]
    async fn apply_reloads_first_then_restarts_running_and_failed_units() {
        let units = FakeUnits::with_states(&[
            ("app-pod.service", "active"),
            ("web.service", "active"),
            ("db.service", "failed"),
            ("idle.service", "inactive"),
        ]);
        sync(
            &units,
            &[],
            &[
                "db.container",
                "idle.container",
                "new.container",
                "web.container",
                "app.pod",
            ],
            &[],
        )
        .reload_and_apply()
        .await;
        assert_eq!(
            units.log(),
            [
                "reload",
                "restart app-pod.service",
                "restart db.service",
                "restart web.service",
            ]
        );
    }

    #[tokio::test]
    async fn a_member_changed_with_its_pod_restarts_only_through_the_pod() {
        let units = FakeUnits::with_states(&[
            ("app-pod.service", "active"),
            ("web.service", "active"),
            ("solo.service", "active"),
        ])
        .wanted(&[
            ("app-pod.service", "default.target"),
            ("web.service", "app-pod.service"),
        ]);
        sync(
            &units,
            &[],
            &["solo.container", "web.container", "app.pod"],
            &[],
        )
        .reload_and_apply()
        .await;
        assert_eq!(
            units.log(),
            ["reload", "restart app-pod.service", "restart solo.service"]
        );
    }

    #[tokio::test]
    async fn apply_starts_added_units_that_would_run_after_a_reboot() {
        let units = FakeUnits::with_states(&[
            // Enabled: started.
            ("enabled.service", "inactive"),
            // No [Install] and no pod: only placed on disk.
            ("plain.service", "inactive"),
            // Joins a running pod: started.
            ("member.service", "inactive"),
            ("app-pod.service", "active"),
            // Joins a stopped pod: left for the pod to bring up.
            ("sleeper.service", "inactive"),
            ("idle-pod.service", "inactive"),
            // Already up (e.g. something else started it): left alone.
            ("up.service", "active"),
        ])
        .wanted(&[
            ("enabled.service", "default.target"),
            ("member.service", "app-pod.service"),
            ("sleeper.service", "idle-pod.service"),
            ("up.service", "default.target"),
        ]);
        sync(
            &units,
            &[],
            &[],
            &[
                "enabled.container",
                "member.container",
                "never-loaded.container",
                "plain.container",
                "sleeper.container",
                "up.container",
            ],
        )
        .reload_and_apply()
        .await;
        assert_eq!(
            units.log(),
            ["reload", "start enabled.service", "start member.service"]
        );
    }

    #[tokio::test]
    async fn members_of_a_pod_started_or_restarted_here_come_up_with_it() {
        let units = FakeUnits::with_states(&[
            ("new-pod.service", "inactive"),
            ("a.service", "inactive"),
            ("app-pod.service", "active"),
            ("b.service", "inactive"),
        ])
        .wanted(&[
            ("new-pod.service", "default.target"),
            ("a.service", "new-pod.service"),
            ("b.service", "app-pod.service"),
        ]);
        sync(
            &units,
            &[],
            &["app.pod"],
            &["a.container", "b.container", "new.pod"],
        )
        .reload_and_apply()
        .await;
        assert_eq!(
            units.log(),
            ["reload", "restart app-pod.service", "start new-pod.service",]
        );
    }

    #[tokio::test]
    async fn apply_without_changes_or_additions_does_not_reload() {
        let units = FakeUnits::with_states(&[("web.service", "active")]);
        sync(&units, &["web.container"], &[], &[])
            .reload_and_apply()
            .await;
        assert!(units.log().is_empty());
    }

    #[tokio::test]
    async fn a_failed_reload_restarts_and_starts_nothing() {
        let units = FakeUnits {
            fail_reload: true,
            ..FakeUnits::with_states(&[("web.service", "active"), ("new.service", "inactive")])
                .wanted(&[("new.service", "default.target")])
        };
        sync(&units, &[], &["web.container"], &["new.container"])
            .reload_and_apply()
            .await;
        assert_eq!(units.log(), ["reload"]);
    }

    #[tokio::test]
    async fn wait_until_stopped_gives_up_on_a_stuck_unit() {
        let units = FakeUnits::with_states(&[("web.service", "deactivating")]);
        assert!(!wait_until_stopped(&units, "web.service", Duration::from_millis(10)).await);

        units
            .states
            .lock()
            .unwrap()
            .insert("web.service".into(), "inactive".into());
        assert!(wait_until_stopped(&units, "web.service", Duration::from_millis(10)).await);
    }

    #[tokio::test]
    async fn plan_without_unit_control_plans_nothing() {
        let plan = UnitSync::plan(None, "g", Path::new("/nonexistent"), "a", "b").await;
        assert_eq!(plan.changes, UnitChanges::default());
        let plan = UnitSync::plan_clone(None, "g", Path::new("/nonexistent")).await;
        assert_eq!(plan.changes, UnitChanges::default());
    }
}
