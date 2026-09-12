//! Best-effort host-dependency diagnostics, surfaced in the UI as warning
//! banners (see `web::templates::health_banners`) rather than failure modes.
//! Nothing here blocks startup or a request: each check degrades to "don't
//! know" rather than asserting something that might be wrong.
//!
//! The snapshot is computed once at startup and held in `AppState` behind a
//! [`HealthCell`], so it doesn't just sit frozen for the process lifetime --
//! Settings' "System" card can re-run both checks on demand (its Refresh
//! button) after an admin action like `loginctl enable-linger` or
//! installing podman, and every page picks up the new result immediately
//! since they all read through the same cell.

use std::path::Path;
use std::sync::{Arc, RwLock};

/// Snapshot of the host dependencies sooth relies on but doesn't strictly
/// require to start: podman's quadlet generator (used for best-effort
/// create/edit validation, see `quadlet::writer`) and linger (whether this
/// user's systemd session, and therefore sooth itself when run as its
/// service, survives logout).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Health {
    pub podman_generator_found: bool,
    /// `None` when it couldn't be determined (rather than guessing) -- see
    /// `linger_enabled`.
    pub linger_enabled: Option<bool>,
}

impl Health {
    pub fn check() -> Self {
        Self {
            podman_generator_found: crate::quadlet::writer::generator_present(),
            linger_enabled: linger_enabled(),
        }
    }

    /// Whether any checklist item needs attention -- drives the notice dot
    /// on the sidebar's Settings icon (see `web::templates::shell`), so it's
    /// kept in one place rather than re-deriving the same two conditions at
    /// every call site that only cares "is something wrong", not what.
    pub fn has_warning(&self) -> bool {
        !self.podman_generator_found || self.linger_enabled == Some(false)
    }
}

/// A shared, refreshable `Health` snapshot held in `AppState`. `Health`
/// itself stays a plain `Copy` value -- this is just the "current one, and
/// a way to replace it" wrapper, kept separate so every page's template
/// signature can go on taking a plain `Health` rather than a lock.
///
/// A `std::sync::RwLock` is plenty here: reads are a lock + copy of two
/// small fields, `refresh()` is only ever invoked from the Settings page's
/// Refresh button (a rare, explicit action, not a hot path), and both
/// checks are themselves quick filesystem stats -- nothing here is worth
/// `.await`ing over, so the sync std lock is simpler than reaching for
/// `tokio::sync::RwLock`.
#[derive(Clone)]
pub struct HealthCell(Arc<RwLock<Health>>);

impl HealthCell {
    pub fn new(initial: Health) -> Self {
        Self(Arc::new(RwLock::new(initial)))
    }

    /// The current snapshot -- whatever the last `check()` or `refresh()`
    /// found. A poisoned lock (only possible if a prior reader/writer
    /// panicked mid-access) still yields the last-known value rather than
    /// panicking again here; a stale health check is never worth crashing
    /// a request over.
    pub fn get(&self) -> Health {
        *self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Re-runs both checks and stores the fresh result as the new current
    /// snapshot, returning it so the caller can render it immediately
    /// without a second `get()`.
    pub fn refresh(&self) -> Health {
        let fresh = Health::check();
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = fresh;
        fresh
    }
}

/// Whether `loginctl enable-linger` has been run for the current user, i.e.
/// whether the systemd user manager (and everything under it, sooth
/// included when deployed as its own `--user` service) keeps running after
/// logout rather than being torn down.
///
/// Checked via the plain marker file systemd itself writes
/// (`/var/lib/systemd/linger/<user>`) rather than a D-Bus round trip to
/// `org.freedesktop.login1`, which lives on the *system* bus -- a second
/// connection sooth would otherwise have no reason to hold. Returns `None`,
/// rather than a possibly-wrong `false`, when the username can't be
/// determined (e.g. a launcher that clears `$USER`/`$LOGNAME`).
fn linger_enabled() -> Option<bool> {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()?;
    Some(Path::new("/var/lib/systemd/linger").join(user).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    // `generator_present`/`linger_enabled` read fixed host paths directly
    // (no injection point), so they're exercised manually rather than here
    // -- see the mount-namespace trick in the project notes for that. What
    // *is* worth pinning down is `has_warning`'s truth table, since that's
    // the pure logic gating the banner and the sidebar notice dot.
    #[test]
    fn no_warning_when_generator_found_and_linger_enabled() {
        let health = Health {
            podman_generator_found: true,
            linger_enabled: Some(true),
        };
        assert!(!health.has_warning());
    }

    #[test]
    fn no_warning_when_linger_status_is_unknown() {
        // Unknown must never read as a warning -- that would be claiming
        // something's wrong when we simply couldn't tell.
        let health = Health {
            podman_generator_found: true,
            linger_enabled: None,
        };
        assert!(!health.has_warning());
    }

    #[test]
    fn warns_when_generator_missing() {
        let health = Health {
            podman_generator_found: false,
            linger_enabled: Some(true),
        };
        assert!(health.has_warning());
    }

    #[test]
    fn warns_when_linger_disabled() {
        let health = Health {
            podman_generator_found: true,
            linger_enabled: Some(false),
        };
        assert!(health.has_warning());
    }

    #[test]
    fn warns_when_both_checks_fail() {
        let health = Health {
            podman_generator_found: false,
            linger_enabled: Some(false),
        };
        assert!(health.has_warning());
    }

    #[test]
    fn cell_get_returns_the_initial_value() {
        let initial = Health {
            podman_generator_found: true,
            linger_enabled: Some(true),
        };
        let cell = HealthCell::new(initial);
        assert_eq!(cell.get(), initial);
    }

    #[test]
    fn cell_refresh_stores_what_it_returns() {
        // `Health::check()` reads real host paths, so its actual result
        // varies by machine -- this only pins down that whatever `refresh`
        // computes is also what a subsequent `get` sees, not any specific
        // value. A bug that recomputes but forgets to store (or vice versa)
        // would desync these.
        let cell = HealthCell::new(Health::default());
        let refreshed = cell.refresh();
        assert_eq!(cell.get(), refreshed);
    }
}
