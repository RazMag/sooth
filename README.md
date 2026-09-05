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

## Running

```sh
# One-time: generate a password hash (prompts twice, hidden input)
cargo run -- --hash-password

# Then run the server
SOOTH_AUTH_PASSWORD_HASH='<hash from above>' cargo run
```

By default it binds `127.0.0.1:8420` and serves `static/` relative to the
current working directory — run it from the repo root, or set
`WorkingDirectory=` if deployed as a systemd unit (see below).

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
WorkingDirectory=%h/path/to/sooth
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
2. Log in at `/login`; confirm the dashboard lists whatever's already in the
   quadlet directory with correct status.
3. Create a test quadlet via "New unit"; confirm it lands on disk and
   `systemctl --user list-unit-files` shows the generated `.service`.
4. Start it; confirm the status badge flips to `active/running` live
   (within ~1s, no page refresh), cross-checked with `systemctl --user status`.
5. Enable it; confirm `UnitFileState` matches `systemctl --user is-enabled`.
6. Open the log viewer; compare against `journalctl --user -u <name>.service`.
7. Stop, disable, delete; confirm the file and generated unit are gone.
8. Restart `sooth` while a unit is running; confirm status is correctly
   re-derived from systemd, not stale (there's no cache to go stale).
9. Submit an intentionally invalid unit file; confirm a clear validation
   error with no partial write, and check the error id shows up in
   `journalctl --user -u sooth` if deployed as a service.

## Development

```sh
cargo test      # parser/naming/writer unit tests
cargo clippy --all-targets
```
