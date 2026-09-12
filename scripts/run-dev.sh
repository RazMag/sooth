#!/bin/sh
#
# Runs sooth against a scratch quadlet directory instead of your real
# ~/.config/containers/systemd, so you can poke at the dashboard without
# touching anything real. Builds the binary, seeds one demo quadlet file
# (unless --no-seed), and either prompts for a password via
# `sooth --hash-password` or uses one you pass with --hash.
#
# POSIX sh, no fish required.
#
# Usage:
#   scripts/run-dev.sh                        # prompts for a password, fresh scratch dir
#   scripts/run-dev.sh --hash '$argon2id$...'  # skip the prompt
#   scripts/run-dev.sh --dir /path/to/dir     # reuse an existing scratch dir
#   scripts/run-dev.sh --port 8123 --release --no-seed
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
#   scripts/run-dev.sh --fake-no-podman
#   scripts/run-dev.sh --fake-linger-disabled
#   scripts/run-dev.sh --fake-no-podman --fake-linger-disabled   # both checklist items fail at once

set -u

usage() {
    echo "Usage: run-dev.sh [--dir <path>] [--port <n>] [--hash <argon2-hash>] [--release] [--no-seed]"
    echo "                  [--fake-no-podman] [--fake-linger-disabled]"
    echo ""
    echo "  --fake-no-podman        Hide podman's quadlet generator (in a bwrap sandbox,"
    echo "                          not on your real system) so the health check reads"
    echo "                          \"Not found\"."
    echo "  --fake-linger-disabled  Same idea for the linger marker, so it reads"
    echo "                          \"Disabled\" regardless of your real linger state."
}

dir_flag=
port_flag=
hash_flag=
release_flag=
no_seed_flag=
fake_no_podman_flag=
fake_linger_disabled_flag=

while [ $# -gt 0 ]; do
    case "$1" in
        -d|--dir)
            [ $# -ge 2 ] || { echo "error: $1 requires a value" >&2; exit 1; }
            dir_flag=$2
            shift 2
            ;;
        --dir=*)
            dir_flag=${1#--dir=}
            shift
            ;;
        -p|--port)
            [ $# -ge 2 ] || { echo "error: $1 requires a value" >&2; exit 1; }
            port_flag=$2
            shift 2
            ;;
        --port=*)
            port_flag=${1#--port=}
            shift
            ;;
        --hash)
            [ $# -ge 2 ] || { echo "error: $1 requires a value" >&2; exit 1; }
            hash_flag=$2
            shift 2
            ;;
        --hash=*)
            hash_flag=${1#--hash=}
            shift
            ;;
        --release)
            release_flag=1
            shift
            ;;
        --no-seed)
            no_seed_flag=1
            shift
            ;;
        --fake-no-podman)
            fake_no_podman_flag=1
            shift
            ;;
        --fake-linger-disabled)
            fake_linger_disabled_flag=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "error: unknown option '$1'" >&2
            usage >&2
            exit 1
            ;;
    esac
done

repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd) || exit 1
cd -- "$repo_root" || exit 1

profile=debug
cargo_flags=
if [ -n "$release_flag" ]; then
    profile=release
    cargo_flags=--release
fi

# `build.rs` rebuilds the frontend automatically when frontend/** changed and
# node_modules is present -- nothing to do here. Run `npm run watch` in another
# pane for sub-second rebuilds while iterating on the UI.
echo "==> building (cargo build $cargo_flags)"
cargo build $cargo_flags || exit 1

bin=$repo_root/target/$profile/sooth

port=8420
if [ -n "$port_flag" ]; then
    port=$port_flag
fi

if [ -n "$dir_flag" ]; then
    scratch=$dir_flag
    mkdir -p -- "$scratch"
else
    scratch=$(mktemp -d)
    echo "==> scratch quadlet dir: $scratch"
fi

if [ -z "$no_seed_flag" ] && [ ! -e "$scratch/demo.container" ]; then
    echo "==> seeding $scratch/demo.container"
    printf '[Container]\nImage=docker.io/library/alpine\nExec=sleep infinity\n' > "$scratch/demo.container"
fi

if [ -n "$hash_flag" ]; then
    hash=$hash_flag
else
    echo "==> no --hash given, generating one now"
    hash=$("$bin" --hash-password) || exit 1
fi

sandbox=
if [ -n "$fake_no_podman_flag" ] || [ -n "$fake_linger_disabled_flag" ]; then
    if ! command -v bwrap >/dev/null 2>&1; then
        echo "error: --fake-no-podman/--fake-linger-disabled need bubblewrap (bwrap) installed" >&2
        exit 1
    fi
    sandbox="bwrap --unshare-user --unshare-pid --uid $(id -u) --gid $(id -g) --bind / / --dev /dev --proc /proc"
    if [ -n "$fake_no_podman_flag" ]; then
        echo "==> --fake-no-podman: hiding podman's quadlet generator in a bwrap sandbox (real system untouched)"
        sandbox="$sandbox --tmpfs /usr/lib/systemd/user-generators --tmpfs /usr/lib/systemd/system-generators"
    fi
    if [ -n "$fake_linger_disabled_flag" ]; then
        echo "==> --fake-linger-disabled: hiding the linger marker in a bwrap sandbox (real system untouched)"
        sandbox="$sandbox --tmpfs /var/lib/systemd/linger"
    fi
fi

echo "==> starting sooth on http://127.0.0.1:$port  (quadlet dir: $scratch)"
env SOOTH_QUADLET_DIR="$scratch" \
    SOOTH_AUTH_PASSWORD_HASH="$hash" \
    SOOTH_BIND_ADDR="127.0.0.1:$port" \
    $sandbox "$bin"
