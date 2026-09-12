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
| `events.rs` | `DashboardEvent` (`Status { service, status }` / `UnitsChanged`) on an app-wide `broadcast` channel. Independent of `systemd`/`web`. |
| `quadlet/` | Disk side. `discovery` (**recursive** enumerate/parse/load, `find_in_tree`/`list_groups`, recursive fs watch), `parser` (INI → ordered `Section`s, keeps duplicate keys), `model` (`QuadletUnit` incl. `group` + `rel_path()`, `UnitKind`), `naming` (file name ↔ service name, `valid_stem`, `valid_group`, `compose_rel_path`, `basename`), `writer` (validate + atomic write + generator dry-run, `move_file` / `move_dir` / `delete_dir` — groups are **not** auto-pruned when emptied), `ports` (`PublishPort=` parse + collision detection), `envfile` (`env/<stem>.env` sidecar + managed `EnvironmentFile=` line patching — **group-independent**, always keyed by stem under `env/`), `install` (`[Install]` section = rootless "enable": `set_enabled` raw-text patch of `WantedBy=default.target`). |
| `systemd/` | D-Bus side. `client` (`org.freedesktop.systemd1.Manager` proxy: start/stop/restart/reload/status + environment get/set/unset), `status` (fetch `UnitStatus` via `Properties.GetAll`), `watch` (subscribe to `PropertiesChanged` for every unit, filter to sooth-managed, re-fetch + broadcast). **No enable/disable D-Bus call**: podman's `.service` units live under a systemd generator dir, which `EnableUnitFiles` rejects ("transient or generated"). "Enable"/"disable" is `quadlet::install::set_enabled` patching the file's `[Install]` section + `client.reload()`. "Enabled" state is **not** read from the file (it can name a target that doesn't exist / isn't in the login path) but from systemd's computed `WantedBy=`/`RequiredBy=` reverse deps — `UnitStatus::is_autostart_enabled` is true iff `default.target` is among them. `UnitFileState` is unusable here: systemd always reports `generated` for these. |
| `hostenv.rs` | The user manager's `${NAME}` environment. Owns `~/.config/environment.d/50-sooth.conf`; reads other `*.conf` there read-only. |
| `journal.rs` | The one shell-out in the app: `journalctl --user -u <service>` for the log tail + live follow. |
| `web/routes.rs` | Router assembly. `mount_unit_routes` registers the shared per-unit routes (detail/actions/start/…/edit/delete/**move**/logs) at **six** prefixes: `/containers /pods /volumes /networks /images /units`. Plus the group-directory routes (`handlers::groups` → `core::*group*`): `POST /groups` (`mkdir` an empty group, or a subgroup with a `parent` field), `/groups/move` (re-parent), `/groups/rename` (rename the leaf), `/groups/delete` (rmdir — refused while any unit lives under it). |
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
  a subgroup.
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

## Commit style

Conventional commits with a scope: `feat(web): …`, `fix(web): …`,
`build(frontend): …`, `docs: …`. Do not commit, merge, or push unless the
user explicitly asks.
