# Changelog

All notable changes to sooth are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and sooth uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.3.2]: https://github.com/RazMag/sooth/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/RazMag/sooth/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/RazMag/sooth/compare/v0.2.1...v0.3.0
