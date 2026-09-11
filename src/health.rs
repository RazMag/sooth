//! Best-effort host-dependency diagnostics, surfaced in the UI as warning
//! banners (see `web::templates::health_banners`) rather than failure modes.
//! Nothing here blocks startup or a request: each check degrades to "don't
//! know" rather than asserting something that might be wrong, and the
//! snapshot is computed once and held in `AppState` for the process
//! lifetime -- these are host facts that don't change without an admin
//! action and a restart (enabling linger, installing podman).

use std::path::Path;

/// Snapshot of the host dependencies sooth relies on but doesn't strictly
/// require to start: podman's quadlet generator (used for best-effort
/// create/edit validation, see `quadlet::writer`) and linger (whether this
/// user's systemd session, and therefore sooth itself when run as its
/// service, survives logout).
#[derive(Debug, Clone, Copy, Default)]
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
}
