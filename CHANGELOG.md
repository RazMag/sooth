# Changelog

All notable changes to sooth are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and sooth uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.5] - 2026-09-27

### Added

- **Unit logs grouped by run**: each restart starts a new run, marked by a
  "New run" divider. A "Latest run only" toggle (on by default, remembered
  per browser) hides earlier runs' output, including live lines as soon as
  the new run logs. Timestamps are shown in your browser's local time.

### Changed

- The Ports screen's Owner column is now **Container**: a port published by
  a pod names the member container(s) serving it, based on their
  `ExposePort=` or their image's `EXPOSE`. A member whose image isn't pulled
  yet gets a "Pull image" button.
- Port collision notes name the conflicting units and flag only the
  colliding mapping, not every port of the file.

### Fixed

- Live log lines containing carriage returns (e.g. progress bars) no longer
  gain extra blank lines; they render the same as after a reload.

## [0.3.4] - 2026-09-23

### Fixed

- Long values in a unit's detail-page Overview card (file path, a nested
  group's name, secret and host variable names with their "missing" badges)
  no longer spill past the card's edge. Secrets and host variables are now
  listed one per line.

## [0.3.3] - 2026-09-23

### Added

- **Missing host variables** are flagged like missing secrets: a unit that
  references a `${NAME}` the systemd user manager doesn't have gets a
  "missing" badge on its detail page and its Git Sync card, with a link that
  opens the Environment page's add form prefilled with the name.
- **Missing secrets and host variables in the unit tables**: each row shows
  an "N missing" badge naming what isn't set.

## [0.3.2] - 2026-09-23

### Added

- **GitHub token status** on the Settings page and the Git Sync add form:
  shows whether a token is in use (with its last 4 characters), none is
  saved, or a newly saved/removed token is waiting for a restart.

### Changed

- The GitHub access token setting moved into Settings' "Security &
  sessions" section.

### Fixed

- Adding a git-sync with no group chosen silently did nothing (the browser
  logged "The invalid form control with name='group' is not focusable");
  it now opens the group picker instead. An empty required group field
  reads "choose a group…" rather than "root".

## [0.3.1] - 2026-09-23

### Added

- **Secrets page** (`/secrets`) over this user's podman secret store: add,
  replace, and delete secrets, and see which units use each one. Values are
  piped to podman on stdin and never logged. Each row shows a masked value
  with an eye button to show or hide it, a copy button, and Replace.
- **Restart on replace**: saving a secret can restart the units using it
  (running and failed ones; stopped units stay stopped), and each row has a
  restart action for doing it later.
- **Missing-secret warnings**: a `Secret=` that names a secret podman doesn't
  have is flagged on the Secrets page (with a Set action), on the container's
  detail page, and as a "N missing" badge on the Git Sync card of the group
  it's in.
- The editor's insert panel lists stored secrets and inserts a
  `Secret=NAME,type=env,target=NAME` line on click.
- README section on keeping secrets out of git-synced repos: commit only
  `Secret=` names and set the values on the host.

### Changed

- Deleting a secret is refused while any quadlet still references it.

## [0.3.0] - 2026-09-23

### Added

- **Pod resource forms**: the New/Edit Pod pages can attach existing
  containers, networks, and volumes, or define new ones inline. Each new
  resource is saved as its own quadlet file and wired into the pod in one
  submission.
- **Edit pod members in place**: the Edit Pod page has a Members section with
  an editor for every container in the pod, saved to each container's own
  file.
- **Private Git Sync repos**: a GitHub access token set on the Settings page
  authenticates `https://github.com/...` remotes. The token never appears on
  a command line or in the checkout's `.git/config`, and is only sent to
  github.com.
- **Move or rename a synced group** in place; the sync keeps running under
  the new path.
- Sidebar shows the running version, and the theme toggle gains a "System"
  option that follows the OS light/dark preference.
- Startup log includes a clickable URL alongside the bind address.

### Changed

- "+ Add group" opens a small popup instead of expanding the toolbar.
- The host-variable panel floats beside the editor on every New/Edit page.
- The Git Sync form's group field uses the same picker as the pod forms.

### Fixed

- The detail page's move-to-group dropdown now closes on an outside click.

[0.3.5]: https://github.com/RazMag/sooth/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/RazMag/sooth/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/RazMag/sooth/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/RazMag/sooth/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/RazMag/sooth/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/RazMag/sooth/compare/v0.2.1...v0.3.0
