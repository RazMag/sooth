//! The Activity feed: what happened to this machine's units, newest first,
//! kept in `activity.jsonl` next to the config file so it survives a
//! restart and moves with the config folder.
//!
//! Entries are derived, not reported: [`spawn`] watches the dashboard's
//! event channel and records a unit failing, recovering, starting or
//! stopping, and a git sync updating or failing. The first status seen for
//! a unit after startup only sets its baseline, so a restart never floods
//! the feed with "started".

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::AppState;
use crate::events::DashboardEvent;
use crate::quadlet::UnitKind;
use crate::quadlet::gitsync::SyncState;
use crate::systemd::UnitStatus;

/// How many entries the feed keeps. The file is compacted back to this many
/// once it holds twice as many lines.
const KEEP: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Something broke: a unit failed, a sync errored.
    Problem,
    /// Something changed: started, stopped, recovered, synced.
    Change,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds.
    pub at: u64,
    pub kind: Kind,
    /// The unit or sync the entry is about, shown in bold before `text`.
    pub subject: String,
    pub text: String,
}

#[derive(Debug, Default)]
struct Inner {
    entries: VecDeque<Entry>,
    /// Lines in the file, compacted lines included.
    lines: usize,
}

/// The feed, shared by every clone. Reads come from memory; each append is
/// also written to the file (best effort -- a write failure is logged, and
/// the entry still shows until the next restart).
#[derive(Clone)]
pub struct ActivityLog {
    path: Arc<PathBuf>,
    inner: Arc<Mutex<Inner>>,
}

impl ActivityLog {
    /// Loads the feed from `path`, skipping lines that don't parse. A
    /// missing file is an empty feed.
    pub fn load(path: PathBuf) -> Self {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
        let mut entries: VecDeque<Entry> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        while entries.len() > KEEP {
            entries.pop_front();
        }
        Self {
            path: Arc::new(path),
            inner: Arc::new(Mutex::new(Inner { entries, lines })),
        }
    }

    /// `activity.jsonl` in the folder holding `config_path`.
    pub fn beside_config(config_path: &Path) -> Self {
        let dir = config_path.parent().unwrap_or(Path::new("."));
        Self::load(dir.join("activity.jsonl"))
    }

    pub fn push(&self, entry: Entry) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.entries.push_back(entry.clone());
        while inner.entries.len() > KEEP {
            inner.entries.pop_front();
        }
        let result = if inner.lines + 1 >= KEEP * 2 {
            let compacted = rewrite(&self.path, &inner.entries);
            inner.lines = inner.entries.len();
            compacted
        } else {
            inner.lines += 1;
            append(&self.path, &entry)
        };
        if let Err(e) = result {
            warn!(path = %self.path.display(), error = %e, "could not save the activity feed");
        }
    }

    /// Up to `limit` entries, newest first.
    pub fn recent(&self, limit: usize) -> Vec<Entry> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.entries.iter().rev().take(limit).cloned().collect()
    }
}

fn append(path: &Path, entry: &Entry) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(entry)?)
}

fn rewrite(path: &Path, entries: &VecDeque<Entry>) -> std::io::Result<()> {
    let mut text = String::new();
    for e in entries {
        text.push_str(&serde_json::to_string(e)?);
        text.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The settled states worth comparing; transitional ones (activating,
/// deactivating, reloading) are skipped until they land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    Up,
    Down,
    Failed,
}

fn settle(status: &UnitStatus) -> Option<Settled> {
    if status.is_failed() {
        Some(Settled::Failed)
    } else if status.is_active() {
        Some(Settled::Up)
    } else if status.active_state == "inactive" {
        Some(Settled::Down)
    } else {
        None
    }
}

/// What a unit moving `from` -> `to` is worth saying, if anything.
fn unit_entry(from: Settled, to: Settled, status: &UnitStatus) -> Option<(Kind, String)> {
    use Settled::*;
    match (from, to) {
        (a, b) if a == b => None,
        (_, Failed) => Some((
            Kind::Problem,
            match status.failure.as_deref() {
                Some(f) if f.starts_with("exit ") => {
                    format!("exited with status {}", &f["exit ".len()..])
                }
                Some(f) => format!("failed ({f})"),
                None => "failed".to_string(),
            },
        )),
        (Failed, Up) => Some((Kind::Change, "recovered".to_string())),
        (_, Up) => Some((Kind::Change, "started".to_string())),
        (Up, Down) => Some((Kind::Change, "stopped".to_string())),
        // Failed -> inactive is a reset-failed, not news.
        _ => None,
    }
}

/// What a sync moving to `to` is worth saying, given its last settled state.
fn sync_entry(from: Option<&SyncState>, to: &SyncState) -> Option<(Kind, String)> {
    match (from, to) {
        (Some(SyncState::UpToDate { commit: old }), SyncState::UpToDate { commit })
            if old != commit =>
        {
            Some((
                Kind::Change,
                format!("updated {} → {}", short(old), short(commit)),
            ))
        }
        (Some(SyncState::Error(_)), SyncState::UpToDate { commit }) => {
            Some((Kind::Change, format!("back in sync at {}", short(commit))))
        }
        (Some(SyncState::UpToDate { .. }) | None, SyncState::Error(msg)) => {
            Some((Kind::Problem, format!("failed: {msg}")))
        }
        _ => None,
    }
}

fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// Records entries from the event channel until the channel closes.
/// Returns the task's handle so `main` can abort it on shutdown.
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    let log = state.activity.clone();
    let events = state.events.clone();
    let git_sync = state.git_sync.clone();
    // Subscribe before taking the baseline, so no change falls in between.
    let mut rx = events.subscribe();
    tokio::spawn(async move {
        // Every unit's current state is the baseline; a unit first seen
        // later (just created) counts as having been down.
        let mut units: HashMap<String, Settled> =
            crate::web::core::load_units_for_kinds(&state, &UnitKind::all())
                .await
                .unwrap_or_default()
                .iter()
                .filter_map(|(u, s)| Some((u.service_name(), settle(s)?)))
                .collect();
        // Only settled sync states (up to date / error) are remembered.
        let mut syncs: HashMap<String, SyncState> = git_sync
            .snapshot()
            .into_iter()
            .filter(|(_, s)| matches!(s.state, SyncState::UpToDate { .. } | SyncState::Error(_)))
            .map(|(c, s)| (c.group, s.state))
            .collect();
        let mut recorded = false;
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            match event {
                DashboardEvent::Status { service, status } => {
                    let Some(to) = settle(&status) else { continue };
                    let subject = service
                        .strip_suffix(".service")
                        .unwrap_or(&service)
                        .to_string();
                    let from = units.insert(service, to).unwrap_or(Settled::Down);
                    if let Some((kind, text)) = unit_entry(from, to, &status) {
                        log.push(Entry {
                            at: now_secs(),
                            kind,
                            subject,
                            text,
                        });
                        recorded = true;
                    }
                }
                DashboardEvent::GitSyncChanged => {
                    for (config, status) in git_sync.snapshot() {
                        if !matches!(
                            status.state,
                            SyncState::UpToDate { .. } | SyncState::Error(_)
                        ) {
                            continue;
                        }
                        let from = syncs.get(&config.group);
                        if let Some((kind, text)) = sync_entry(from, &status.state) {
                            log.push(Entry {
                                at: now_secs(),
                                kind,
                                subject: format!("Git Sync {}/", config.group),
                                text,
                            });
                            recorded = true;
                        }
                        syncs.insert(config.group, status.state);
                    }
                }
                _ => {}
            }
            if std::mem::take(&mut recorded) {
                let _ = events.send(DashboardEvent::ActivityChanged);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u64) -> Entry {
        Entry {
            at: n,
            kind: Kind::Change,
            subject: format!("u{n}"),
            text: "started".into(),
        }
    }

    #[test]
    fn feed_survives_a_reload_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("activity.jsonl");
        let log = ActivityLog::load(path.clone());
        log.push(entry(1));
        log.push(entry(2));
        let again = ActivityLog::load(path);
        let got: Vec<u64> = again.recent(10).iter().map(|e| e.at).collect();
        assert_eq!(got, vec![2, 1]);
    }

    #[test]
    fn feed_compacts_to_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("activity.jsonl");
        let log = ActivityLog::load(path.clone());
        for n in 0..(KEEP as u64 * 2 + 3) {
            log.push(entry(n));
        }
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert!(lines < KEEP * 2, "{lines}");
        let again = ActivityLog::load(path);
        assert_eq!(again.recent(1)[0].at, KEEP as u64 * 2 + 2);
        assert_eq!(again.recent(usize::MAX).len(), KEEP);
    }

    #[test]
    fn unit_transitions_read_as_sentences() {
        let mut failed = UnitStatus::not_found();
        failed.failure = Some("exit 1".into());
        use Settled::*;
        assert_eq!(
            unit_entry(Up, Failed, &failed),
            Some((Kind::Problem, "exited with status 1".into()))
        );
        let ok = UnitStatus::not_found();
        assert_eq!(
            unit_entry(Failed, Up, &ok),
            Some((Kind::Change, "recovered".into()))
        );
        assert_eq!(
            unit_entry(Down, Up, &ok),
            Some((Kind::Change, "started".into()))
        );
        assert_eq!(
            unit_entry(Up, Down, &ok),
            Some((Kind::Change, "stopped".into()))
        );
        assert_eq!(unit_entry(Failed, Down, &ok), None);
        assert_eq!(unit_entry(Up, Up, &ok), None);
    }

    #[test]
    fn sync_transitions_read_as_sentences() {
        let up = |c: &str| SyncState::UpToDate { commit: c.into() };
        assert_eq!(
            sync_entry(Some(&up("a1b2c3e999")), &up("d4e5f61000")),
            Some((Kind::Change, "updated a1b2c3e → d4e5f61".into()))
        );
        assert_eq!(sync_entry(Some(&up("a")), &up("a")), None);
        assert_eq!(
            sync_entry(Some(&up("a")), &SyncState::Error("diverged".into())),
            Some((Kind::Problem, "failed: diverged".into()))
        );
        assert_eq!(
            sync_entry(
                Some(&SyncState::Error("x".into())),
                &SyncState::Error("y".into())
            ),
            None
        );
    }
}
