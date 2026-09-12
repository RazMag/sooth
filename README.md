# sooth

A web dashboard for managing rootless [Podman Quadlets][quadlet] — no
database, no separate agent, no state of its own. The quadlet directory on
disk *is* the state; systemd's D-Bus API is used directly for
start/stop/enable/disable and live status. sooth is a single binary with its
CSS/JS embedded, so it runs from any directory.

[quadlet]: https://docs.podman.io/en/latest/markdown/podman-systemd.unit.5.html

## Scope

- **Rootless only.** It manages `$XDG_CONFIG_HOME/containers/systemd` (usually
  `~/.config/containers/systemd`) and talks to `systemctl --user`'s D-Bus
  session — not the system-wide quadlet locations.
- Template units (`name@.container`) are listed read-only; instantiate them
  with the CLI.
- Subdirectories of the quadlet directory *are* descended into (they're the
  "groups" a unit can be filed under); `.d/` drop-in directories are not, and
  are not merged into what's shown.

## Features

- **Services** landing page — Containers and Pods combined, with a
  total/running/failed stat bar.
- Dedicated **Volumes**, **Networks**, and **Images** (Image + Build units)
  sections, plus a generic `/units` listing that covers every kind and is the
  only home for `.kube` units.
- **Ports** — every declared `PublishPort=` across Containers and Pods, with
  conflicting host ports flagged.
- **Environment** — the host `${NAME}` variables a quadlet file can
  interpolate. sooth manages its own set through an `environment.d` drop-in
  (`~/.config/environment.d/50-sooth.conf`) and also pushes each change to
  the running user manager over D-Bus, so it applies without a re-login.
  Variables from other `environment.d` files are shown read-only.
- **Live status** over Server-Sent Events: status badges, stat counts, and
  action buttons update within about a second of a real systemd state change,
  no page refresh. Externally-edited quadlet files are picked up the same way.
- **Lifecycle actions** — start, stop, restart, enable, disable — issued
  straight over systemd's D-Bus manager interface.
- **Create / edit** for every kind is a raw INI editor (CodeMirror,
  syntax-highlighted, live-validated against the exact check the write path
  runs). The file-name field takes a bare stem; the extension comes from the
  section (`/units/new` has a kind picker). A collapsible panel lists the
  host `${NAME}` variables for one-click insertion.
- `.container` / `.build` create and edit also carry a Name/Value environment
  editor: sooth writes the variables to a sidecar
  `<quadlet_dir>/env/<name>.env` and keeps a managed `EnvironmentFile=` line
  pointing at it (added when there are variables, removed when there are
  none; any other `EnvironmentFile=` lines are left alone).
- **Groups** — file quadlets into subdirectories of the quadlet directory
  (`media/`, `infra/db/`, …) from the UI: an optional field on the New form,
  and a "Move" control on every unit's detail page and row menu. Podman
  recurses into these subdirectories and the name has no effect on the
  generated unit, so a move is a plain file rename — the service keeps running.
  The list tables group by directory into collapsible sections (root units
  first, then one section per group; collapsed state is remembered per
  browser). `.d/` drop-in directories and the `env/` sidecar dir are still
  skipped.
- Writes are atomic (temp file + `rename`) and validated up front — including
  a best-effort dry-run against the real podman quadlet generator — so a
  reader never sees a partial file and an invalid submission never lands.
- **Log viewer** with a live tail (`journalctl --user -u <unit>`).
- Single-operator **auth**: one argon2-hashed password, `HttpOnly` +
  `SameSite=Strict` session cookie, CSRF tokens on every mutating form.
  Sessions are in-memory, so restarting the process signs everyone out.
- Light / dark theme with no flash on reload; the sidebar collapses to a
  drawer on narrow viewports.

## Install

### Requirements

- **Linux with systemd**, used as a normal (non-root) user with a working
  user session bus — `systemctl --user` must function (enable lingering, or
  run it from an active login session).
- **Podman with Quadlet support** (4.4+). The podman quadlet generator
  (`/usr/lib/systemd/user-generators/podman-user-generator`) is used for
  dry-run validation when present; sooth still works without it, relying on
  its own structural checks.
- **A Rust stable toolchain**, 2024 edition (rustc 1.85 or newer), to build.
- **Node.js 22+** only if you want to rebuild the frontend assets —
  `static/style.css` and `static/app.js` are committed, and `cargo build`
  falls back to them when no Node toolchain is present.

### Build and run

```sh
cargo build --release

# One-time: generate a password hash (prompts twice, hidden input)
./target/release/sooth --hash-password

# Then run the server
SOOTH_AUTH_PASSWORD_HASH='<hash from above>' ./target/release/sooth
```

By default it binds `127.0.0.1:8420`.

### Configuration

Layered, later wins: built-in defaults → an optional TOML file →
`SOOTH_`-prefixed environment variables. The TOML file defaults to
`~/.config/sooth/config.toml`, overridable with `SOOTH_CONFIG`. Env vars
winning last suits deploying sooth itself as a systemd user service with
secrets in an `EnvironmentFile=`.

| Variable | Default | Purpose |
|---|---|---|
| `SOOTH_AUTH_PASSWORD_HASH` | *(required)* | An argon2 PHC hash from `--hash-password`. Startup fails fast if missing or invalid. |
| `SOOTH_BIND_ADDR` | `127.0.0.1:8420` | Listen address. |
| `SOOTH_QUADLET_DIR` | *(auto-detected)* | Override the quadlet directory (mainly for testing). |
| `SOOTH_COOKIE_SECURE` | `false` | Require HTTPS for the session cookie. Set `true` when reachable off-box, behind a TLS-terminating reverse proxy. |
| `SOOTH_LOG_FILTER` | `sooth=info,tower_http=info,zbus=warn` | A `tracing-subscriber` `EnvFilter` string; `RUST_LOG` also works. |
| `SOOTH_SESSION_IDLE_TIMEOUT_SECS` | `43200` (12h) | Session idle expiry. |

All of these are also editable from the in-app Settings page, which writes
back to the resolved TOML file; changes there take effect on the next
restart. The Settings page can also set a new login password (it verifies
the current one, then writes the new hash to the same file). A field pinned
by a `SOOTH_*` variable is shown read-only there, since the environment
layer would override the saved value on the next start anyway. A **Restart**
button on the same page winds the server down and re-execs the binary in
place (same argv and environment) so those saved changes take effect without
shell access — it doesn't rely on a systemd `Restart=` or a known unit name.

### Run as a systemd user service

```ini
# ~/.config/systemd/user/sooth.service
[Unit]
Description=sooth dashboard

[Service]
EnvironmentFile=%h/.config/sooth/sooth.env
ExecStart=%h/path/to/sooth/target/release/sooth

[Install]
WantedBy=default.target
```

`sooth.env` holds `SOOTH_AUTH_PASSWORD_HASH=...` and any other overrides.
Logs go to the user journal automatically (`journalctl --user -u sooth`),
since systemd captures unit stdout by default.

## Development

`static/style.css` and `static/app.js` are committed build artifacts
(Tailwind v4 + esbuild + CodeMirror 6, from `frontend/**`). `cargo build`
embeds them via `rust-embed`, so a build with no Node toolchain still
produces a working binary from whatever is checked in.

When Node *is* set up, `build.rs` re-runs the frontend build on
`cargo build` / `cargo run`, but only when something under `frontend/**` (or
`package.json` / the build script) changed since the last build.

```sh
npm ci            # once, to enable frontend rebuilds
cargo run         # rebuilds static/ from frontend/** if needed, then serves

cargo test        # parser / naming / writer / ports / envfile unit tests
cargo clippy --all-targets
cargo fmt --check
```

Iterating on the UI: run `npm run watch` in a second pane — it rewrites
`static/` on save and a debug build re-reads it per request, so you don't
restart the server. Without watch, restart `cargo run` to pick up a
`frontend/**` edit. `SOOTH_SKIP_FRONTEND_BUILD=1` skips the frontend build
entirely (e.g. a read-only checkout).

`scripts/run-dev.fish` builds and runs sooth against a throwaway scratch
quadlet directory (seeded with one demo unit) instead of your real
`~/.config/containers/systemd`, so you can poke at the dashboard without
touching anything real:

```sh
scripts/run-dev.fish                 # prompts for a password, fresh scratch dir
scripts/run-dev.fish --port 8123 --dir /tmp/sooth-scratch --no-seed
```

`--fake-no-podman` and `--fake-linger-disabled` exercise the Settings "System"
card and the dashboard warning banner (see `src/health.rs`) by running sooth
inside a `bwrap` sandbox that hides just the relevant path -- your real
system is never touched:

```sh
scripts/run-dev.fish --fake-no-podman --fake-linger-disabled
```

CI (`.github/workflows/ci.yml`) runs two jobs: `frontend` rebuilds the
assets and fails if `static/` is stale (`git diff --exit-code`), and `rust`
runs `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, and
`cargo test` with `SOOTH_SKIP_FRONTEND_BUILD=1`.

### Manual smoke test

1. `sooth --hash-password`, set `SOOTH_AUTH_PASSWORD_HASH`, run it, log in.
2. Confirm Services shows correct total/running/failed counts and each
   sidebar section lists what's already in the quadlet directory.
3. Create a container from the Services "+ Container" editor; confirm live
   validation (`✓ / ✗`), that it lands on disk with the INI you typed, and
   `systemctl --user list-unit-files` shows the generated `.service`.
4. Start it from the kebab menu; confirm the badge and stat bar flip to
   `Running` live, cross-checked with `systemctl --user status`.
5. Open the log viewer; compare against `journalctl --user -u <name>.service`.
6. Stop, disable, delete; confirm the file and generated unit are gone and
   the counts update without a reload.
7. Submit an intentionally invalid file (e.g. missing `[Container]`); confirm
   a clear error with entered content preserved and no partial write.
