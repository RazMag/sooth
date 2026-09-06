# sooth

A web dashboard for managing rootless [Podman Quadlets][quadlet] — no
database, no separate agent. The quadlet directory on disk *is* the state,
and systemd's D-Bus API is used directly for start/stop/enable/disable and
live status.

[quadlet]: https://docs.podman.io/en/latest/markdown/podman-systemd.unit.5.html

## Scope

- **Rootless only**: manages `$XDG_CONFIG_HOME/containers/systemd` (usually
  `~/.config/containers/systemd`) and talks to `systemctl --user`'s D-Bus
  session, not the system-wide/root quadlet locations.
- Template units (`name@.container`) are listed read-only; instantiating
  them is out of scope for now, use the CLI.
- `.d/` drop-in directories are not managed or merged into what's shown.

## Sections

The UI is organized as a sidebar with **Services** (the landing page —
Containers + Pods combined, with a total/running/failed stat bar),
**Volumes**, **Networks**, **Images** (Image + Build units), **Ports**
(every declared `PublishPort=` across Containers/Pods, with conflicting host
ports flagged), and **Environment** — the host variables a quadlet file can
interpolate as `${NAME}`. sooth manages its own set through an
`environment.d` drop-in (`~/.config/environment.d/50-sooth.conf`) and also
pushes each add/remove to the running user manager over D-Bus so it applies
without a re-login; variables configured in other `environment.d` files are
shown read-only, and the inherited base environment is not listed. Create
and edit for every kind use a raw INI editor
(CodeMirror, syntax-highlighted, live-validated against the same check the
write path runs); its file-name field takes just a stem and the extension
comes from the section (`/units/new` offers a kind picker), and a
collapsible panel beside it lists the host `${NAME}` variables for one-click
insertion. `.container` / `.build` create and edit also carry a Name/Value
environment editor: sooth writes the variables to a sidecar
`<quadlet_dir>/env/<name>.env` and keeps a managed `EnvironmentFile=` line
pointing at it in the unit's primary section (added when there are variables,
removed when there are none; other `EnvironmentFile=` lines are left alone).
`.kube` units have no dedicated section and are only reachable via the
generic `/units` listing (unlinked from the sidebar, also usable as a full
cross-kind fallback view).

## Running

```sh
# One-time: generate a password hash (prompts twice, hidden input)
cargo run -- --hash-password

# Then run the server
SOOTH_AUTH_PASSWORD_HASH='<hash from above>' cargo run
```

By default it binds `127.0.0.1:8420`. Static assets (CSS/JS) are embedded in
the binary, so it can run from any directory.

## Configuration

Layered as: built-in defaults → optional TOML file → `SOOTH_`-prefixed
environment variables (env wins). The TOML file defaults to
`~/.config/sooth/config.toml`, overridable via `SOOTH_CONFIG`.

| Variable | Default | Purpose |
|---|---|---|
| `SOOTH_AUTH_PASSWORD_HASH` | *(required)* | An argon2 PHC hash from `--hash-password`. Startup fails fast if missing/invalid. |
| `SOOTH_BIND_ADDR` | `127.0.0.1:8420` | Listen address. |
| `SOOTH_QUADLET_DIR` | *(auto-detected)* | Override the quadlet directory (mainly for testing). |
| `SOOTH_COOKIE_SECURE` | `false` | Require HTTPS for the session cookie. Set `true` when reachable off-box, behind a TLS-terminating reverse proxy. |
| `SOOTH_LOG_FILTER` | `sooth=info,tower_http=info,zbus=warn` | A `tracing-subscriber` `EnvFilter` string; `RUST_LOG` also works. |
| `SOOTH_SESSION_IDLE_TIMEOUT_SECS` | `43200` (12h) | Session idle expiry. |

Sessions are in-memory (no database) — restarting the process signs
everyone out.

## Deploying as a systemd user service

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

## Manual verification checklist

1. `cargo run -- --hash-password`, set `SOOTH_AUTH_PASSWORD_HASH`, `cargo run`.
2. Log in; confirm Services shows correct total/running/failed counts and each
   sidebar section lists whatever's already in the quadlet directory.
3. Create a container via the Services page's "+ Container" editor; confirm
   live validation (`✓ / ✗`), that it lands on disk with the INI you typed,
   and `systemctl --user list-unit-files` shows the generated `.service`.
   Repeat for a pod, then use its "Add container to this pod" link to
   confirm `Pod=` prefills correctly on the container form.
4. Start it from the list page's kebab menu; confirm the status badge flips
   to `Running` live (within ~1s, no page refresh) both there and in the
   Services stat bar, cross-checked with `systemctl --user status`.
5. Enable it; confirm the `enabled` chip matches `systemctl --user is-enabled`.
6. Open the log viewer; compare against `journalctl --user -u <name>.service`.
7. Check the Ports section shows the container's published port(s); create a
   second unit on the same host port and confirm both are flagged as
   conflicting.
8. Stop, disable, delete; confirm the file and generated unit are gone, and
   the Services counts update without a page reload.
9. Restart `sooth` while a unit is running; confirm status is correctly
   re-derived from systemd, not stale (there's no cache to go stale).
10. Submit an intentionally invalid file (e.g. missing `[Container]`); confirm
    a clear validation error with entered content preserved and no partial write.
11. Create a `.kube` file directly in the quadlet directory; confirm it's
    manageable at `/units/<file>` even though it has no sidebar section.
12. Toggle the theme (sidebar footer) and shrink the window below ~960px;
    confirm no flash on reload and the sidebar collapses to a drawer.
13. On the Environment page add `app_path=/srv/app`; confirm it appears under
    "Managed by sooth", lands in `~/.config/environment.d/50-sooth.conf`, and
    shows up in `systemctl --user show-environment`. Reference `${app_path}`
    in a `Volume=` line of a new container and confirm the generated unit
    resolves it. Remove it and confirm it's gone from both the file and the
    live manager environment.
14. On `/containers/new` the name field is a stem plus a fixed `.container`
    suffix; type `webapp`, add env rows `FOO=bar` and `BAZ=${FOO}`, submit.
    Confirm `webapp.container` gains `EnvironmentFile=%h/.config/containers/systemd/env/webapp.env`
    in `[Container]`, the sidecar holds both lines, and
    `systemctl --user show webapp.service -p Environment` resolves them. Edit
    it, delete every env row, save → the sidecar and the `EnvironmentFile=`
    line are gone and nothing else changed; add a row back → the line
    reappears at the end of `[Container]`. Delete the unit → the sidecar goes
    too. On `/units/new` the kind picker drives the suffix, and `.build` is
    the way to reach a build unit's env editor. A bad name (`../evil`) or a
    bad variable (`1BAD=x`) redisplays the form with the text preserved and
    nothing written.

## Development

`static/style.css` and `static/app.js` are committed build artifacts
(Tailwind v4 + esbuild, from `frontend/**`). `cargo build` embeds them, so a
build with no Node toolchain still produces a working binary from whatever is
checked in.

When Node *is* set up (`npm ci` once), `build.rs` re-runs the frontend build
on `cargo build` / `cargo run`, but only when something under `frontend/**`
(or `package.json` / the build script) changed since the last build.

```sh
npm ci            # once
cargo run         # a cold start rebuilds static/ from frontend/** if needed, then serves

cargo test        # parser/naming/writer/ports unit tests
cargo clippy --all-targets
```

Iterating on the UI: run `npm run watch` in a second pane — it rewrites
`static/` on save and a debug build re-reads it per request, so you don't
restart the server. Without watch, restart `cargo run` to pick up a
`frontend/**` edit. `SOOTH_SKIP_FRONTEND_BUILD=1` skips the frontend build
entirely (e.g. a read-only checkout).
