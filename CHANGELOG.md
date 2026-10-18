# Changelog

All notable changes to sooth are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and sooth uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Port assignments and host-network ports survive a git sync.** Since
  0.6.0 these were saved as `X-Sooth-ServesPort=` / `X-Sooth-HostPort=`
  lines in the container's quadlet file. In a git-synced directory that is
  a local edit, and the next sync that brought in a new commit reset the
  file and threw it away. Sooth now keeps them in its own file,
  `.sooth/port-notes.toml` in the quadlet directory, so quadlet files are no
  longer touched and nothing is reloaded when you change them. A unit's
  entry is dropped when the unit goes: deleted in sooth, deleted upstream
  and removed by a git sync, or removed along with its git sync. Lines
  written by 0.6.0 or 0.6.1 are no longer read: assign those ports again on
  the Ports screen, and delete the old lines if you like.
- **A container you stop no longer shows as failed.** A container that
  takes longer than podman's stop timeout to shut down (or ignores SIGTERM)
  is killed, exits with status 137, and systemd marks it failed. When sooth
  stops a unit and the stop leaves it failed, sooth now resets it to
  inactive. The exit status is still in the unit's log. A unit that crashes
  on its own still shows as failed. Containers already showing as failed
  stay that way until you start them again.

## [0.6.1] - 2026-10-17

### Added

- **List the ports of host-network containers.** A container with
  `Network=host` uses the host's ports directly, so it has no
  `PublishPort=` lines and sooth used to show no ports for it. The Ports
  screen now has a "Host network" table listing these containers, where
  you can add the ports each one listens on (`8080`, `53/udp` or a range
  like `8000-8010`) and remove them again. They then appear in the ports
  table, flagged like any other port if they clash with another unit's
  published port, and as `8080 host` pills on the Services list and the
  container's page. That page also links to the Ports screen to add or
  change them. This only changes what sooth shows: the ports are saved as
  `X-Sooth-HostPort=` lines in the container's `[Unit]` section, which
  podman and systemd ignore, so nothing is restarted.

## [0.6.0] - 2026-10-16

### Added

- **Assign a pod's published ports to its containers.** On the Ports screen,
  each port a pod publishes now shows which of its containers it reaches,
  with a small control to change that. "auto" matches the container that
  declares the port (`ExposeHostPort=` or its image's `EXPOSE`). Picking a
  container marks the port "assigned" to it, and the choice wins over any
  automatic match. A port no container declares stays on the pod and reads
  "assign…". The Services list puts each port on the row of the container it
  is assigned to. Assigning only changes what sooth shows: it is saved as an
  `X-Sooth-ServesPort=` line in the container's `[Unit]` section, which
  podman and systemd ignore, so nothing is restarted.

- **See why a port is shown where it is.** An ⓘ icon next to each pod
  port's container shows a tooltip on hover or keyboard focus, saying
  whether the port was assigned by hand, matched by `ExposeHostPort=`, or
  matched by the image's `EXPOSE` (naming the image). The Services list's
  port pills give the same explanation on hover.

- **A pod member's page lists its ports.** A container in a pod can't
  publish ports of its own, so its Overview "Ports" used to stay empty. It
  now lists the pod's ports that reach this container, each with its ⓘ
  reason, names the pod, and links to the Ports screen to change them.

## [0.5.0] - 2026-10-12

### Added

- **Check a container's image for updates** from its row's ⋯ menu. "Check
  for update" asks the registry whether the image's tag now points to a newer
  image, without downloading it. If there is one, an **Update** button pulls
  it and restarts the container so it runs the new image; a stopped container
  stays stopped and uses the new image on its next start. Images pinned to a
  digest and local (`localhost/`) images can't be checked, and containers
  built from a `.build` file don't get the option.

- **Git Sync keeps running units in step with the repo**: when a sync pulls
  new configuration for a container, pod or kube unit (its quadlet file or a
  `.d/` drop-in), sooth restarts that unit so the change takes effect. Only
  running or failed units are restarted; stopped ones stay stopped. A newly
  added unit is started if it would be running after a reboot anyway: it is
  enabled in its `[Install]` section, or it joins a running pod. That
  includes the units a newly added sync clones. A pod's changes, such as a
  new `PublishPort=`, apply by restarting the pod together with its
  containers, each restarted only once. When a unit
  is deleted from the repo, sooth stops it before removing its file, so its
  container is cleaned up rather than left running. Volumes and networks are
  never stopped or removed, and their data is kept.

### Fixed

- A row's ⋯ menu no longer runs off the page: near the bottom of a long list
  it opens upward, and it stays inside the window when a wide table is
  scrolled sideways.

## [0.4.1] - 2026-10-04

### Changed

- Auto-update has its own icon (an upward arrow in a circle), on list rows and
  the detail page, so it no longer looks like the restart action.

### Fixed

- In Firefox, the tree's branch lines no longer break above and below a tall
  row, such as one with a long port list.

## [0.4.0] - 2026-10-04

### Added

- **Pods and Directories layouts** for every unit table (Services, Volumes,
  Networks, Images, All units), switched from the toolbar and remembered per
  browser.
  - **Pods** (the default) draws a branch tree. Each pod heads the units it
    owns, a resource several pods share shows as a linked leaf under each of
    them, and everything else hangs off a "Standalone" heading.
  - **Directories** shows your group folders, with a chip naming each unit's
    pod. Dragging units between groups and "Add group" live here.
- A pod row on the Services table counts its running containers, updated
  live.
- A container's row links the volumes, networks, and image or build it uses.
- The Services table has an Image column; the Images table has a Used by
  column.

### Changed

- Each row starts with a colored kind icon, replacing the Kind and Type
  columns.
- Pod and folder rows are shaded and connected to their units by branch
  lines. The table sits in a rounded card.
- Autostart and auto-update show as small icons next to a row's status.
  Hover them for details.
- Volumes list "Used by" before "Driver", and Networks list "Subnet" first.
- A pod's Ports cell says "4 served by members" instead of "4 ports on its
  containers" for the ports shown on its member containers.

## [0.3.6] - 2026-10-04

### Added

- **Pods in every unit table**: a pod row now heads the units it owns: its
  member containers, plus any volume, network, image or build that only that
  pod uses. Shared or unused resources stay at the top level. The Volumes,
  Networks, Images and All units tables show a pod whenever it owns something
  there. Pod rows collapse separately from groups (expanded by default,
  remembered per browser), and filtering keeps a matching unit's pod visible.
- **Logo and favicon**: a wand-sparkles mark beside the wordmark (sidebar,
  mobile top bar, login page) and as a favicon that follows the light/dark
  accent color.

### Changed

- On the Services list, a port a pod publishes now shows on the member
  container that serves it. Only ports no member serves stay on the pod row,
  with a note saying how many moved. A pod's detail page still lists all of
  its ports.
- A pod row shows its unit count by kind ("2 containers, 1 volume") on its
  second line instead of as a pill next to the name.
- Long port lists fold after four mappings behind a "+N more" toggle, in the
  tables and on the detail page's Overview. File names no longer wrap.
- The product name is capitalized as "Sooth" throughout the UI.

### Fixed

- Matching a pod's port to the member that serves it now reads
  `ExposeHostPort=`. 0.3.5 looked for `ExposePort=`, which isn't a quadlet
  key, so it always fell back to the image's `EXPOSE` or to every member.
- Filtering now shows matching units inside collapsed groups.

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

[Unreleased]: https://github.com/RazMag/sooth/compare/v0.6.1...HEAD
[0.6.1]: https://github.com/RazMag/sooth/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/RazMag/sooth/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/RazMag/sooth/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/RazMag/sooth/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/RazMag/sooth/compare/v0.3.6...v0.4.0
[0.3.6]: https://github.com/RazMag/sooth/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/RazMag/sooth/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/RazMag/sooth/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/RazMag/sooth/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/RazMag/sooth/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/RazMag/sooth/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/RazMag/sooth/compare/v0.2.1...v0.3.0
