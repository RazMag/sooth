# AGENTS.md

Working notes for AI coding agents in this repo. Keep this file in sync when
the architecture or conventions below change.

## What this is

`sooth` is a rootless Podman Quadlet dashboard: an Axum web server that reads
`~/.config/containers/systemd/*.{container,volume,network,pod,kube,build,image}`
and drives the units they generate through systemd's **user** D-Bus manager.
There is **no database and no cache** — the quadlet directory on disk and
systemd's own state are the single source of truth, re-derived on every
request. See `README.md` for the user-facing description.

Rust 2024 edition, stable toolchain. HTML is server-rendered with `maud`;
interactivity is htmx + SSE plus a small bundled JS layer.

## Commands

```sh
cargo run                 # serve (rebuilds static/ from frontend/** if Node is set up)
cargo test                # unit tests live in-module (#[cfg(test)]); no integration suite
cargo clippy --all-targets
cargo fmt --check
npm ci && npm run build   # rebuild static/style.css + static/app.js after a frontend/** edit
npm run watch             # sub-second static/ rebuilds while iterating; debug server re-reads per request
scripts/run-dev.sh      # run against a throwaway scratch quadlet dir
```

`justfile` wraps these as `just build|release|test|fmt|fmt-check|lint|check`,
`just frontend|watch-frontend`, `just dev [args...]` (forwards to
`scripts/run-dev.sh`), and `just ci` (reproduces
`.github/workflows/ci.yml` locally). `just --list` shows them all. It's a
convenience wrapper only — the raw commands above are still what actually
runs and remain valid with no `just` installed. `rust-toolchain.toml` pins
`stable` + `rustfmt`/`clippy`, so a bare `rustup`-managed `cargo` already
resolves the right toolchain.

Requires a working `systemctl --user` session bus at runtime (the server
opens `Connection::session()` on startup and exits if it fails).

## Module map (`src/`)

| Path | Responsibility |
|---|---|
| `main.rs` | Startup: load config, connect D-Bus, spawn the status watch + fs watch tasks, serve, cancel tasks on shutdown. Also the `--hash-password` CLI. |
| `config.rs` | `Config` (figment: defaults → TOML → `SOOTH_` env) and `AppState` (the `Arc`-wrapped handles every handler gets). |
| `error.rs` | `AppError` + `PageError` (full-page) / `FragmentError` (htmx inline) wrappers. The **only** place an error becomes an HTTP response; every handler returns `Result<_, PageError|FragmentError>` and uses `?`. |
| `events.rs` | `DashboardEvent` (`Status { service, status }` / `UnitsChanged` / `GitSyncChanged` / `SelfUpdateChanged`) on an app-wide `broadcast` channel. Independent of `systemd`/`web`. |
| `quadlet/` | Disk side. `discovery` (**recursive** enumerate/parse/load, `find_in_tree`/`list_groups`, recursive fs watch), `parser` (INI → ordered `Section`s, keeps duplicate keys), `model` (`QuadletUnit` incl. `group` + `rel_path()`, `UnitKind`), `naming` (file name ↔ service name, `valid_stem`, `valid_group`, `compose_rel_path`, `basename`), `writer` (validate + atomic write + generator dry-run, `move_file` / `move_dir` / `delete_dir` — groups are **not** auto-pruned when emptied), `ports` (`PublishPort=` parse + collision detection), `envfile` (`env/<stem>.env` sidecar + managed `EnvironmentFile=` line patching — **group-independent**, always keyed by stem under `env/`), `install` (`[Install]` section = rootless "enable": `set_enabled` raw-text patch of `WantedBy=default.target`), `gitsync` (a group directory mirrored from a git remote: `git` holds the shell-out wrappers, `manager` holds `GitSyncManager` — one poll task per configured sync, live add/remove, see its Gotchas note below). |
| `selfupdate/` | Checks GitHub Releases for a newer `sooth` binary and downloads it. `mod.rs` holds the config/status types (`UpdateMode::Off\|Notify\|Auto`); `manager` holds `SelfUpdateManager` — one poll task (there's only ever one target: sooth itself), live-reconfigurable, "Check now"/"Download update" wake it early exactly like git-sync's "Sync now". `Notify` mode stops at `UpdateState::ReadyToRestart` once downloaded; installing it is just the Settings page's ordinary Restart button, not a distinct self-update action. `Auto` mode notifies `restart` itself right after downloading. The GitHub API listing, checksum verification, download, and the actual binary swap are all delegated to the `self_update` crate (which uses `self-replace` internally); this module only decides *when* to check and *whether* to download what it finds. See its Gotchas note below on `current_exe`/re-exec. |
| `systemd/` | D-Bus side. `client` (`org.freedesktop.systemd1.Manager` proxy: start/stop/restart/reload/status + environment get/set/unset), `status` (fetch `UnitStatus` via `Properties.GetAll`), `watch` (subscribe to `PropertiesChanged` for every unit, filter to sooth-managed, re-fetch + broadcast). **No enable/disable D-Bus call**: podman's `.service` units live under a systemd generator dir, which `EnableUnitFiles` rejects ("transient or generated"). "Enable"/"disable" is `quadlet::install::set_enabled` patching the file's `[Install]` section + `client.reload()`. "Enabled" state is **not** read from the file (it can name a target that doesn't exist / isn't in the login path) but from systemd's computed `WantedBy=`/`RequiredBy=` reverse deps — `UnitStatus::is_autostart_enabled` is true iff `default.target` is among them. `UnitFileState` is unusable here: systemd always reports `generated` for these. |
| `hostenv.rs` | The user manager's `${NAME}` environment. Owns `~/.config/environment.d/50-sooth.conf`; reads other `*.conf` there read-only. |
| `journal.rs` | Shells out to `journalctl --user -u <service>` for the log tail + live follow. (`quadlet::gitsync::git` is the app's other shell-out, to `git`.) |
| `web/routes.rs` | Router assembly. `mount_unit_routes` registers the shared per-unit routes (detail/actions/start/…/edit/delete/**move**/logs) at **six** prefixes: `/containers /pods /volumes /networks /images /units`. Plus the group-directory routes (`handlers::groups` → `core::*group*`): `POST /groups` (`mkdir` an empty group, or a subgroup with a `parent` field), `/groups/move` (re-parent), `/groups/rename` (rename the leaf), `/groups/delete` (rmdir — refused while any unit lives under it). And the Git Sync routes (`handlers::gitsync` → `GitSyncManager`): `GET /git-sync` (page) + `POST` (add), `/git-sync/rows` (status fragment), `/git-sync/{sync,force,delete}` — each takes its target `group` from the POST body, not a `{group}` path segment, since a nested group's `/` can't live in one. The self-update routes (`handlers::selfupdate` → `SelfUpdateManager`) live under `/settings/self-update` instead of a dedicated page (there's exactly one target, unlike git-sync's arbitrary-many groups): `GET`/`POST` (card fragment + save), `/self-update/check`, `/self-update/download`. Installing a downloaded update posts to the existing `/settings/restart`, not a self-update-specific route. |
| `web/core.rs` | Kind-agnostic business logic (`execute_action`, `create_unit`, `edit_unit`, `delete_unit`, `move_unit`, `load_units_for_kinds`) + `section_path` / `section_index_path` / `unit_url` — the single source of truth for turning a unit into a URL. |
| `web/handlers/` | Thin Axum handlers. Most kinds share one implementation; only `services` (the `/` home = Containers + Pods) and `list` (Volumes/Networks/Images/all) differ, and only by a `ListSpec`. `raw_create` is the one create path for every section. |
| `web/sse.rs` | `/events` stream: renders `DashboardEvent`s as named SSE events (`status-{service}` carries a badge fragment; `units-changed` / `any-status` are `"1"` pings that trigger htmx re-fetches). |
| `web/assets.rs` | `/static/*` from `rust-embed` (baked in for release, read from disk in debug). Content-hash ETag + `Cache-Control: no-cache`. |
| `web/templates/` | `maud` render functions. `mod.rs` has the shell/sidebar (`NavItem`) and shared widgets; sibling modules are per-section. |

## Conventions and invariants

- **File name vs service name.** A quadlet is addressed in URLs by its
  *file name* (`web.container`); systemd is addressed by its *service name*
  (`web.service`, from `naming::service_name`). Never mix them. All
  unit→URL construction goes through `web/core.rs` so this stays structurally
  hard to get wrong.
- **File name vs on-disk path.** A unit may sit in a group subdirectory, so
  its disk path is `<group>/<file_name>` (`QuadletUnit::rel_path()`), but the
  *bare file name* is still the unique key (podman requires it unique
  tree-wide) — URLs, CSS ids, and `discovery::load_by_name` all key off the
  basename. Anything that *writes* (`writer::*`, `core::edit/delete/move`)
  must use `rel_path()`, not `file_name`, or it lands in the wrong directory.
  Group changes go through `core::move_unit` (a rename + reload; the service
  is unchanged). The row menu's move form and the table drag-and-drop
  (`frontend/dragdrop.js`) post `/…/move` over htmx and rely on the
  `units-changed` SSE refresh — only the *detail-page* move redirects. Group
  directories are first-class and **user-managed**: emptying one (moving or
  deleting its last unit) does **not** delete it. `discovery::list_groups`
  walks dirs (not just files) so an empty group still renders as a
  (drop-target) collapsible section; the group-header ⋯ menu and drag re-parent
  drive `/groups/{move,rename,delete}` (delete refused while units remain).
  `writer::move_dir` renames the directory with every unit inside keeping its
  service name; `core::move_group_dir` rejects a move into the group itself or
  a subgroup. A git-synced group (`quadlet::gitsync`) is the one exception to
  "user-managed": `core::synced_destination`/`synced_source` reject filing a
  unit into one, and reject moving/renaming the synced directory itself (or
  an ancestor/descendant of it), since either would fight the next sync or
  break `GitSyncConfig.group`'s path tracking. `templates::list::GroupLists`
  carries both the full group list and the synced subset into the list
  templates so the table can preview the same rule client-side (no drag
  handle on a synced group's header, `dragdrop.js`'s `isSyncedTarget` guard) —
  the server check is still the authoritative one.
- **One implementation, six mount points.** Per-unit behavior does not vary
  by kind — don't add kind-specific handler modules. The detail page
  dispatches on `unit.kind` (the loaded unit's real kind, not the URL
  prefix) inside `handlers/detail.rs`.
- **Edits patch `QuadletUnit.raw` as text**, never re-serialize the parsed
  model — comments and formatting sooth didn't touch must survive. See
  `envfile::patch_environment_file` for the pattern.
- **All writes atomic**: temp file in the same dir → `fsync` → `rename`.
  `writer::write_atomic` and `hostenv`/`envfile` all do this. Validation
  (`writer::validate`, incl. the generator dry-run) runs *before* any write.
- **After any quadlet create/edit/delete**: `systemd.reload()` (daemon-reload
  equivalent) then broadcast `UnitsChanged`. `core::*` already do this.
- **CSRF**: every mutating form carries `csrf_token`; verify with
  `auth::csrf::verify` before touching disk. `raw_create`/`edit_delete`
  verify early because they write the env sidecar before calling `core::*`.
  `/validate` is deliberately *not* CSRF-checked (no state change).
- **No panics in request paths.** `main.rs` sets
  `#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]`.
  Use `?` and the error types. `.unwrap()` is fine in `#[cfg(test)]`.
- **Errors**: return `AppError` variants; pick `PageError` for navigation
  routes and `FragmentError` for htmx action routes. Client-ish quadlet
  errors (`Validation`, `GeneratorRejected`) are `is_client_error()` — those
  redisplay the form with the message and a 422, not a generic error page.
- Tests are colocated `#[cfg(test)] mod tests` blocks. `tempfile` for
  anything touching disk. Keep the pure modules (`parser`, `naming`,
  `ports`, `envfile`) free of D-Bus/HTTP so they stay unit-testable.

## Frontend

- Entry point `frontend/main.js` → `static/app.js` (esbuild, IIFE). Bundles
  htmx, its SSE extension, and CodeMirror 6. `frontend/styles.css` →
  `static/style.css` (Tailwind v4; classes are scanned from
  `src/web/templates`).
- Progressive enhancement only: `initTheme/Filter/Nav/Menus/Logs/Editors/EnvVars`.
  The DOM-scanning ones are idempotent and re-run on `htmx:afterSwap`.
- The code editor (`frontend/editor.js`) keeps the underlying `<textarea>` in
  sync so a normal form submit still carries `contents`, and debounce-posts
  to `/validate` (same check as the write path).
- `static/style.css` and `static/app.js` are **committed**. If you change
  `frontend/**`, run `npm run build` and commit the regenerated assets — CI
  fails on a stale `static/`.

## Gotchas

- The SSE handlers (`/events`, `.../logs/stream`) are otherwise-infinite
  streams. They race against `AppState.shutdown` and end when it flips —
  without that, graceful shutdown waits forever on an open browser tab.
- The status-watch and fs-watch tasks loop forever; `main.rs` must
  `.abort()` them on shutdown or `Runtime::drop` hangs and the process never
  exits despite logging that it did.
- `systemd::watch` opens its **own** D-Bus connection — reading raw messages
  off the shared one races zbus's reply dispatcher and can swallow a method
  reply, hanging a concurrent `StartUnit`.
- `discovery::watch` filters inotify events to real content changes
  (`is_content_change`) *and* to paths `load_all` would surface
  (`is_watched_path` — not under `env/` / `*.d` / a dot-path): sooth reads
  these files on every render, and treating reads or sidecar writes as
  changes would make it trigger `daemon-reload` + full UI refresh on its own
  traffic. The watch is **recursive** (group subdirectories).
- The status watch fires for *every* user unit on the bus (the desktop
  session's churn). `discovery::has_quadlet_for_service` gates it to
  sooth-managed units before any broadcast — it now walks the group tree
  each call, which is fine only because the tree is tiny.
- `env/` (the sidecar dir) sits inside `quadlet_dir` but is invisible to
  `discovery` (which skips it, `*.d`, and dot-dirs while recursing, and only
  matches the 7 quadlet extensions) and to the podman generator — keep it
  that way. The sidecar is never moved when a unit changes group.
- `quadlet::gitsync::manager`'s poll tasks loop forever too, like the
  status-watch/fs-watch tasks above — `main.rs` calls `git_sync.abort_all()`
  alongside `.abort()`ing those. It deliberately does **not** call
  `systemd.reload()` or broadcast `UnitsChanged` itself after a sync: its
  writes land inside `quadlet_dir` exactly like an external edit, so
  `discovery::watch` + the existing fs-watch task already reload and refresh
  the UI for it. Don't add a second reload path here.
- `main::run` captures `std::env::current_exe()` **exactly once**, at
  startup, into `exe_path`, and `reexec` takes that same value as a
  parameter instead of calling `current_exe()` itself. Never change this back
  to re-deriving the path at re-exec time: once `selfupdate` has renamed a
  freshly downloaded binary over the running binary's own path,
  `/proc/self/exe` for *this* still-running (now-unlinked) process resolves
  to `"<path> (deleted)"` — a string naming no real file. This is real, not
  theoretical — verified directly on this host: a plain `mv` over a running
  process's own executable makes `std::env::current_exe()` return the
  `(deleted)` form immediately. `reexec` re-resolving the path at shutdown
  time would then fail outright after every self-update, even though the
  file actually sitting at `exe_path` is perfectly fine to exec.
  `selfupdate` itself doesn't need this same care for its *own* swap: the
  `self_update`/`self-replace` crates resolve `current_exe()` internally, but
  do so before any swap has happened in this process's lifetime, which is
  the one case where a fresh resolution is still correct.
- `selfupdate::manager`'s poll task loops forever too, like the others above
  — `main.rs` calls `self_update.abort()` alongside them. Unlike git-sync, a
  successful download doesn't touch `quadlet_dir` at all. Only `Auto` mode
  calls `restart.notify_one()` itself; `Notify` mode stops at
  `ReadyToRestart` and the card's "Install and restart" button posts
  straight to the existing `/settings/restart` action -- there is no
  separate self-update-specific restart mechanism either way.

## Releasing

Trigger is a push to the protected `release` branch (or a manual "Run
workflow" against it, to retry) -- never a tag push, and never an ordinary
push/merge to `main`. The only path onto `release` is a PR from `main`;
never commit to it directly, and never target it from anywhere but `main`,
or the two branches can end up with the same content merged in a different
order.

`.github/workflows/release.yml` jobs, in dependency order:
- `test`: fmt/clippy/test, mirroring `ci.yml`'s `rust` job. Redundant with
  branch protection requiring `ci.yml` green before a merge, deliberately --
  never trust protection alone (misconfigurable, admin-bypassable) to gate
  what ships.
- `version`: reads `Cargo.toml`'s `version` (no tag to type by hand -- this
  is what a push to `release` replaces `git tag && git push` with) and GETs
  `/repos/<owner>/<repo>/releases/tags/v<version>`; a 200 sets
  `already_released`, making the run a **safe no-op** (e.g. a version-bump-less
  merge, or a retry after a partial failure) rather than a re-publish or a
  failure.
- `build`: two native jobs, no cross-compilation (a GitHub-hosted ARM64
  runner builds that target directly), shipping the raw `sooth` binary per
  target -- no archive, since `self_update` (see `crate::selfupdate`)
  matches an asset by the target triple as a substring of its name.
- `publish`: uploads the binaries plus a `SHA256SUMS` file via
  `softprops/action-gh-release`, passing `tag_name`/`target_commitish`
  explicitly (this action creates the tag itself, against the exact commit
  that triggered the run -- no separate `git tag` step needed).
  `generate_release_notes` is deliberately left off -- GitHub's
  generate-release-notes endpoint 500s unreliably, especially with no prior
  tag to diff against (this bit the very first release; see the
  `fix(release)` commit that removed it).

**One-time setup this repo needs** (Settings -> Branches -> Add branch
protection rule -> pattern `release`): require a pull request before
merging, require status checks to pass before merging (select `ci.yml`'s
`frontend` and `rust` jobs), and leave force-pushes/deletions disallowed.
Doable via the `PUT /repos/{owner}/{repo}/branches/release/protection` REST
endpoint (or `gh api` against it) instead of the UI -- check GitHub's
current branch-protection API docs for the exact JSON shape rather than
assuming a remembered one, since this isn't something to get subtly wrong.

## Commit style

Conventional commits with a scope: `feat(web): …`, `fix(web): …`,
`build(frontend): …`, `docs: …`. Do not commit, merge, or push unless the
user explicitly asks.
