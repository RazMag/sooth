#!/usr/bin/env fish
#
# Runs sooth against a scratch quadlet directory instead of your real
# ~/.config/containers/systemd, so you can poke at the dashboard without
# touching anything real. Builds the binary, seeds one demo quadlet file
# (unless --no-seed), and either prompts for a password via
# `sooth --hash-password` or uses one you pass with --hash.
#
# Usage:
#   scripts/run-dev.fish                      # prompts for a password, fresh scratch dir
#   scripts/run-dev.fish --hash '$argon2id$...'  # skip the prompt
#   scripts/run-dev.fish --dir /path/to/dir   # reuse an existing scratch dir
#   scripts/run-dev.fish --port 8123 --release --no-seed
#
# --fake-no-podman / --fake-linger-disabled exercise the Settings "System"
# card and the dashboard warning banner (see src/health.rs) without actually
# uninstalling podman or touching real linger state. Both run sooth inside a
# `bwrap` (bubblewrap) sandbox that tmpfs-hides just the relevant path(s) --
# your real filesystem is never modified. `bwrap --uid/--gid` (not
# `unshare --map-root-user`) is what makes this work: it does the privileged
# setup mounts internally but then drops the process back to your *real* uid
# before exec'ing sooth, so `systemctl --user`'s D-Bus EXTERNAL auth (which
# checks the connecting peer's real credentials) still succeeds. A plain
# `unshare --map-root-user` leaves the process looking like uid 0 to itself,
# which is enough to do the tmpfs mounts too, but breaks that D-Bus handshake
# ("EXTERNAL rejected by the server") since the claimed identity (0) no
# longer matches the real peer uid.
#
#   scripts/run-dev.fish --fake-no-podman
#   scripts/run-dev.fish --fake-linger-disabled
#   scripts/run-dev.fish --fake-no-podman --fake-linger-disabled   # both checklist items fail at once

argparse 'd/dir=' 'p/port=' 'hash=' 'release' 'no-seed' 'fake-no-podman' 'fake-linger-disabled' 'h/help' -- $argv
or exit 1

if set -q _flag_help
    echo "Usage: run-dev.fish [--dir <path>] [--port <n>] [--hash <argon2-hash>] [--release] [--no-seed]"
    echo "                    [--fake-no-podman] [--fake-linger-disabled]"
    echo ""
    echo "  --fake-no-podman        Hide podman's quadlet generator (in a bwrap sandbox,"
    echo "                          not on your real system) so the health check reads"
    echo "                          \"Not found\"."
    echo "  --fake-linger-disabled  Same idea for the linger marker, so it reads"
    echo "                          \"Disabled\" regardless of your real linger state."
    exit 0
end

set -l repo_root (realpath (dirname (status --current-filename))/..)
cd $repo_root

set -l profile debug
set -l cargo_flags
if set -q _flag_release
    set profile release
    set cargo_flags --release
end

# `build.rs` rebuilds the frontend automatically when frontend/** changed and
# node_modules is present -- nothing to do here. Run `npm run watch` in another
# pane for sub-second rebuilds while iterating on the UI.
echo "==> building (cargo build $cargo_flags)"
cargo build $cargo_flags
or exit 1

set -l bin $repo_root/target/$profile/sooth

set -l port 8420
if set -q _flag_port
    set port $_flag_port
end

set -l scratch
if set -q _flag_dir
    set scratch $_flag_dir
    mkdir -p $scratch
else
    set scratch (mktemp -d)
    echo "==> scratch quadlet dir: $scratch"
end

if not set -q _flag_no_seed
    if not test -e $scratch/demo.container
        echo "==> seeding $scratch/demo.container"
        printf '[Container]\nImage=docker.io/library/alpine\nExec=sleep infinity\n' > $scratch/demo.container
    end
end

set -l hash
if set -q _flag_hash
    set hash $_flag_hash
else
    echo "==> no --hash given, generating one now"
    set hash ($bin --hash-password)
    or exit 1
end

set -l sandbox
if set -q _flag_fake_no_podman; or set -q _flag_fake_linger_disabled
    if not command -q bwrap
        echo "error: --fake-no-podman/--fake-linger-disabled need bubblewrap (bwrap) installed" >&2
        exit 1
    end
    set sandbox bwrap --unshare-user --unshare-pid \
        --uid (id -u) --gid (id -g) \
        --bind / / --dev /dev --proc /proc
    if set -q _flag_fake_no_podman
        echo "==> --fake-no-podman: hiding podman's quadlet generator in a bwrap sandbox (real system untouched)"
        set sandbox $sandbox \
            --tmpfs /usr/lib/systemd/user-generators \
            --tmpfs /usr/lib/systemd/system-generators
    end
    if set -q _flag_fake_linger_disabled
        echo "==> --fake-linger-disabled: hiding the linger marker in a bwrap sandbox (real system untouched)"
        set sandbox $sandbox --tmpfs /var/lib/systemd/linger
    end
end

echo "==> starting sooth on http://127.0.0.1:$port  (quadlet dir: $scratch)"
env SOOTH_QUADLET_DIR=$scratch \
    SOOTH_AUTH_PASSWORD_HASH=$hash \
    SOOTH_BIND_ADDR=127.0.0.1:$port \
    $sandbox $bin
