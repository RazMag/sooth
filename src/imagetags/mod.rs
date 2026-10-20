//! Newer version tags for *pinned* containers -- those whose image names a
//! version tag (`nginx:1.25.3`, `postgres:16.4-alpine3.20`) rather than a
//! floating one like `latest`, which the row kebab's "Check for update"
//! covers instead (`imageinfo::check_update`). See [`ImageTagsManager`]
//! for the runtime side, `quadlet::versiontag` for what counts as a version
//! tag and which tags are "newer", and `web::core::switch_image_tag` for
//! moving a container onto one.
//!
//! The one deliberate exception to "no cache" in sooth: a registry's tag
//! list can't be asked for on every render (thousands of tags, several
//! round trips), so the manager keeps the last listing per repository in
//! memory -- never on disk -- and re-lists on its schedule, on "Check now",
//! from the row kebab's check, and for a repository it hasn't listed yet
//! (a container just added, or moved onto a new repository).

pub mod manager;

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub use manager::ImageTagsManager;

fn default_enabled() -> bool {
    true
}

fn default_poll_interval_secs() -> u64 {
    86_400 // a day
}

/// Persisted settings, round-tripped through `Config.image_tags` (an
/// `[image_tags]` TOML table) and applied live by its Settings card, like
/// `selfupdate::SelfUpdateConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageTagsConfig {
    /// Whether the background poll runs. Off still leaves the on-demand
    /// checks ("Check now", the row kebab) working.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_poll_interval_secs")]
    pub poll_interval_secs: u64,
}

impl Default for ImageTagsConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            poll_interval_secs: default_poll_interval_secs(),
        }
    }
}

/// The last full pass, for the Settings card.
#[derive(Debug, Clone, Default)]
pub struct TagsStatus {
    pub checking: bool,
    /// When the last full pass finished.
    pub checked_at: Option<SystemTime>,
    /// Distinct pinned repositories it listed.
    pub repos: usize,
    /// Pinned containers with a newer tag available.
    pub outdated: usize,
    /// `(image, message)` for each listing that failed.
    pub errors: Vec<(String, String)>,
}
