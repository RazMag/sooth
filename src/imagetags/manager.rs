//! [`ImageTagsManager`]: one background poll task that lists the registry
//! tags of every pinned container's repository, plus the in-memory result
//! the list rows read their "newer tag" badge from. Shaped like
//! `selfupdate::SelfUpdateManager` (a `Clone`, `Arc`-backed handle, live
//! reconfigure, "Check now" wakes the loop early).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use futures_util::StreamExt;
use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::AbortHandle;
use tracing::{info, warn};

use crate::events::{DashboardEvent, EventSender};
use crate::quadlet::versiontag::{self, Candidate, Version};
use crate::quadlet::{QuadletUnit, UnitKind, discovery, refs};

use super::{ImageTagsConfig, TagsStatus};

/// Registries are listed this many at a time during a full pass.
const CONCURRENCY: usize = 4;

type ListFuture = Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send>>;
/// `imageinfo::list_tags`, swappable for tests.
type Lister = Arc<dyn Fn(String) -> ListFuture + Send + Sync>;
/// The last listing per repository (see `ImageTagsManager::repos`).
type Listings = HashMap<String, Result<Vec<String>, String>>;

#[derive(Clone)]
pub struct ImageTagsManager {
    config: Arc<RwLock<ImageTagsConfig>>,
    status: Arc<RwLock<TagsStatus>>,
    /// The last listing per repository, keyed by the repository as the
    /// quadlet writes it (the image minus its tag) -- so a container moved
    /// to another tag of the same repository is answered straight away.
    /// Only version-looking tags are kept. `Err` is the listing's failure.
    repos: Arc<RwLock<Listings>>,
    wake: Arc<Notify>,
    abort: Arc<Mutex<Option<AbortHandle>>>,
    config_path: Arc<PathBuf>,
    quadlet_dir: Arc<PathBuf>,
    events: EventSender,
    lister: Lister,
}

impl ImageTagsManager {
    pub fn new(config_path: Arc<PathBuf>, quadlet_dir: Arc<PathBuf>, events: EventSender) -> Self {
        let lister: Lister = Arc::new(|image: String| {
            Box::pin(async move { crate::imageinfo::list_tags(&image).await })
        });
        Self {
            config: Arc::new(RwLock::new(ImageTagsConfig::default())),
            status: Arc::new(RwLock::new(TagsStatus::default())),
            repos: Arc::new(RwLock::new(HashMap::new())),
            wake: Arc::new(Notify::new()),
            abort: Arc::new(Mutex::new(None)),
            config_path,
            quadlet_dir,
            events,
            lister,
        }
    }

    /// Spawns (or respawns) the poll task -- once from `main`, again from
    /// [`configure`](Self::configure).
    pub fn start(&self, config: &ImageTagsConfig) {
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = config.clone();
        let mgr = self.clone();
        let handle = tokio::spawn(async move { mgr.run_loop().await });
        let prev = self
            .abort
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .replace(handle.abort_handle());
        if let Some(prev) = prev {
            prev.abort();
        }
    }

    /// Persists new settings and restarts the poll task with them. A
    /// respawn doesn't re-list early: the loop waits out what's left of the
    /// interval since the last pass.
    pub fn configure(&self, config: ImageTagsConfig) -> std::io::Result<()> {
        crate::config::patch_image_tags(&self.config_path, |c| *c = config.clone())?;
        info!(
            enabled = config.enabled,
            interval = config.poll_interval_secs,
            "image tag check settings changed"
        );
        self.start(&config);
        let _ = self.events.send(DashboardEvent::ImageTagsChanged);
        Ok(())
    }

    /// "Check now": a full pass right away, enabled or not.
    pub fn check_now(&self) {
        self.wake.notify_one();
    }

    /// Aborts the poll task -- `main` calls this at shutdown, like
    /// `SelfUpdateManager::abort`; `run_loop` never exits on its own.
    pub fn abort(&self) {
        if let Some(h) = self.abort.lock().unwrap_or_else(|e| e.into_inner()).take() {
            h.abort();
        }
    }

    pub fn config_snapshot(&self) -> ImageTagsConfig {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn snapshot(&self) -> TagsStatus {
        self.status
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The newer tags known for a pinned `image` (narrowest range first,
    /// `versiontag::newer`); empty when it isn't pinned, nothing newer
    /// exists, or its repository hasn't been listed (or failed to).
    pub fn newer(&self, image: &str) -> Vec<Candidate> {
        let Some((repo, tag)) = versiontag::split_tag(image) else {
            return Vec::new();
        };
        let repos = self.repos.read().unwrap_or_else(|e| e.into_inner());
        match repos.get(repo) {
            Some(Ok(tags)) => versiontag::newer(tag, tags),
            _ => Vec::new(),
        }
    }

    /// Lists `image`'s repository now and stores the result -- the row
    /// kebab's on-demand check. Doesn't broadcast: the caller re-renders
    /// the one badge it affects. `Err` is the listing's failure.
    pub async fn check_image(&self, image: &str) -> Result<Vec<Candidate>, String> {
        let Some((repo, _)) = versiontag::split_tag(image).filter(|_| versiontag::is_pinned(image))
        else {
            return Ok(Vec::new());
        };
        let listed = self.list(image).await;
        let failed = listed.as_ref().err().cloned();
        self.repos
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(repo.to_string(), listed);
        match failed {
            Some(e) => Err(e),
            None => Ok(self.newer(image)),
        }
    }

    async fn list(&self, image: &str) -> Result<Vec<String>, String> {
        let tags = (self.lister)(image.to_string()).await?;
        Ok(tags
            .into_iter()
            .filter(|t| Version::parse(t).is_some())
            .collect())
    }

    async fn run_loop(self) {
        let mut events = self.events.subscribe();
        let mut listening = true;
        let mut forced = false;
        loop {
            let config = self.config_snapshot();
            let interval = Duration::from_secs(config.poll_interval_secs.max(60));
            let since = || self.snapshot().checked_at.and_then(|t| t.elapsed().ok());
            if forced || (config.enabled && since().is_none_or(|s| s >= interval)) {
                self.check_all().await;
            }
            let sleep =
                tokio::time::sleep(since().map_or(interval, |s| interval.saturating_sub(s)));
            tokio::pin!(sleep);
            forced = loop {
                tokio::select! {
                    _ = &mut sleep, if config.enabled => break false,
                    _ = self.wake.notified() => break true,
                    event = events.recv(), if listening => match event {
                        // A unit added or moved onto a repository not yet
                        // listed: list just those, without waiting a day.
                        Ok(DashboardEvent::UnitsChanged) | Err(RecvError::Lagged(_)) => {
                            if config.enabled {
                                self.check_unlisted().await;
                            }
                        }
                        Ok(_) => {}
                        Err(RecvError::Closed) => listening = false,
                    },
                }
            };
        }
    }

    /// A full pass: lists every pinned repository and replaces the stored
    /// listings (dropping repositories no quadlet pins any more).
    async fn check_all(&self) {
        let Some(pinned) = self.pinned() else {
            return;
        };
        self.status
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .checking = true;
        let _ = self.events.send(DashboardEvent::ImageTagsChanged);

        let results = self.list_many(pinned.clone().into_iter().collect()).await;
        let errors: Vec<(String, String)> = results
            .iter()
            .filter_map(|(repo, r)| Some((pinned.get(repo)?.clone(), r.as_ref().err()?.clone())))
            .collect();
        for (image, error) in &errors {
            warn!(image, error, "listing image tags failed");
        }
        *self.repos.write().unwrap_or_else(|e| e.into_inner()) = results.into_iter().collect();
        let outdated = pinned
            .values()
            .filter(|image| !self.newer(image).is_empty())
            .count();
        info!(
            repos = pinned.len(),
            outdated, "checked pinned images for newer tags"
        );
        *self.status.write().unwrap_or_else(|e| e.into_inner()) = TagsStatus {
            checking: false,
            checked_at: Some(SystemTime::now()),
            repos: pinned.len(),
            outdated,
            errors,
        };
        let _ = self.events.send(DashboardEvent::ImageTagsChanged);
    }

    /// Lists only the pinned repositories with no stored listing yet.
    async fn check_unlisted(&self) {
        let Some(pinned) = self.pinned() else {
            return;
        };
        let unlisted: Vec<(String, String)> = {
            let repos = self.repos.read().unwrap_or_else(|e| e.into_inner());
            pinned
                .into_iter()
                .filter(|(r, _)| !repos.contains_key(r))
                .collect()
        };
        if unlisted.is_empty() {
            return;
        }
        let results = self.list_many(unlisted).await;
        self.repos
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .extend(results);
        let _ = self.events.send(DashboardEvent::ImageTagsChanged);
    }

    /// Lists each `(repository, image)`, [`CONCURRENCY`] at a time.
    async fn list_many(
        &self,
        pinned: Vec<(String, String)>,
    ) -> Vec<(String, Result<Vec<String>, String>)> {
        let lists = pinned.into_iter().map(|(repo, image)| {
            let mgr = self.clone();
            async move {
                let listed = mgr.list(&image).await;
                (repo, listed)
            }
        });
        futures_util::stream::iter(lists)
            .buffer_unordered(CONCURRENCY)
            .collect()
            .await
    }

    /// Every pinned repository on disk, with one image naming it; `None`
    /// (logged) when the quadlet directory can't be read.
    fn pinned(&self) -> Option<BTreeMap<String, String>> {
        match discovery::load_all(&self.quadlet_dir) {
            Ok(all) => Some(pinned_images(&all)),
            Err(e) => {
                warn!(error = %e, "image tag check couldn't read the quadlet directory");
                None
            }
        }
    }
}

/// The registry repositories `all`'s containers are pinned to
/// (`versiontag::is_pinned`, through `.image` quadlets like everywhere
/// else; never a local `.build` or `localhost/` image), each with the first
/// image naming it -- a tagged reference is what `imageinfo::list_tags`
/// takes, since a short name is resolved through the local tag.
pub fn pinned_images(all: &[QuadletUnit]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for unit in all.iter().filter(|u| u.kind == UnitKind::Container) {
        let builds = unit
            .section("Container")
            .and_then(|s| s.get("Image"))
            .is_some_and(|i| i.trim().ends_with(".build"));
        let Some(image) = refs::container_image(unit, all).filter(|_| !builds) else {
            continue;
        };
        if !versiontag::is_pinned(&image) || image.starts_with("localhost/") {
            continue;
        }
        if let Some((repo, _)) = versiontag::split_tag(&image) {
            out.entry(repo.to_string()).or_insert(image);
        }
    }
    out
}

#[cfg(test)]
impl ImageTagsManager {
    fn with_lister(mut self, lister: Lister) -> Self {
        self.lister = lister;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
    }

    fn manager(dir: &std::path::Path) -> ImageTagsManager {
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let lister: Lister = Arc::new(|image: String| {
            Box::pin(async move {
                match image.as_str() {
                    "ghcr.io/home-assistant/home-assistant:2026.9.3" => Ok(vec![
                        "2026.9.3".into(),
                        "2026.9.4".into(),
                        "2026.10.2".into(),
                        "stable".into(),
                    ]),
                    _ => Err(format!("no registry for {image}")),
                }
            })
        });
        ImageTagsManager::new(
            Arc::new(dir.join("config.toml")),
            Arc::new(dir.to_path_buf()),
            tx,
        )
        .with_lister(lister)
    }

    #[test]
    fn finds_pinned_repositories_through_image_quadlets() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        write(
            d,
            "ha.container",
            "[Container]\nImage=ghcr.io/home-assistant/home-assistant:2026.9.3\n",
        );
        write(
            d,
            "ha2.container",
            "[Container]\nImage=ghcr.io/home-assistant/home-assistant:2026.8.0\n",
        );
        write(d, "web.container", "[Container]\nImage=nginx:latest\n");
        write(d, "db.container", "[Container]\nImage=pg.image\n");
        write(
            d,
            "pg.image",
            "[Image]\nImage=docker.io/library/postgres:16.4\n",
        );
        write(d, "app.container", "[Container]\nImage=app.build\n");
        write(d, "app.build", "[Build]\nImageTag=localhost/app:1.0\n");
        let all = discovery::load_all(d).unwrap();
        let pinned = pinned_images(&all);
        assert_eq!(
            pinned.keys().collect::<Vec<_>>(),
            [
                "docker.io/library/postgres",
                "ghcr.io/home-assistant/home-assistant",
            ]
        );
    }

    #[tokio::test]
    async fn full_pass_stores_listings_and_answers_per_image() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        write(
            d,
            "ha.container",
            "[Container]\nImage=ghcr.io/home-assistant/home-assistant:2026.9.3\n",
        );
        write(d, "x.container", "[Container]\nImage=example.com/x:1.0\n");
        let mgr = manager(d);
        mgr.check_all().await;

        let status = mgr.snapshot();
        assert_eq!((status.repos, status.outdated), (2, 1));
        assert_eq!(status.errors.len(), 1);
        assert!(status.checked_at.is_some() && !status.checking);

        let tags: Vec<String> = mgr
            .newer("ghcr.io/home-assistant/home-assistant:2026.9.3")
            .into_iter()
            .map(|c| c.tag)
            .collect();
        assert_eq!(tags, ["2026.9.4", "2026.10.2"]);
        // Another tag of the same repository is answered from the listing.
        let tags: Vec<String> = mgr
            .newer("ghcr.io/home-assistant/home-assistant:2026.10.2")
            .into_iter()
            .map(|c| c.tag)
            .collect();
        assert!(tags.is_empty());
        assert!(mgr.newer("example.com/x:1.0").is_empty());
    }

    #[tokio::test]
    async fn on_demand_check_reports_failures_and_unlisted_are_filled_in() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mgr = manager(d);
        assert!(mgr.check_image("example.com/x:1.0").await.is_err());
        assert!(mgr.check_image("nginx:latest").await.unwrap().is_empty());

        write(
            d,
            "ha.container",
            "[Container]\nImage=ghcr.io/home-assistant/home-assistant:2026.9.3\n",
        );
        mgr.check_unlisted().await;
        assert_eq!(
            mgr.newer("ghcr.io/home-assistant/home-assistant:2026.9.3")
                .len(),
            2
        );
    }
}
