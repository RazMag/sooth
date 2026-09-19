#!/bin/sh
#
# Downloads the latest sooth release binary and installs it as a systemd
# --user service. Meant to be piped into sh:
#
#   curl -fsSL https://raw.githubusercontent.com/RazMag/sooth/main/install.sh | sh
#
# To pass flags through a pipe, use `sh -s --`:
#
#   curl -fsSL .../install.sh | sh -s -- --no-start
#
# Every flag also has an env var equivalent, for piped invocations that
# would rather not deal with `sh -s --`:
#
#   SOOTH_VERSION=v0.1.0 curl -fsSL .../install.sh | sh
#
# Re-run anytime to update: it re-downloads the binary, rewrites the unit,
# and restarts the service. An existing password hash in sooth.env is kept
# unless --hash/--reset-password (or SOOTH_HASH/SOOTH_RESET_PASSWORD) is given.
#
# To remove everything it installed:
#
#   curl -fsSL .../install.sh | sh -s -- --uninstall          # keeps sooth.env
#   curl -fsSL .../install.sh | sh -s -- --uninstall --purge  # also deletes it
#
# POSIX sh, no fish required. Needs curl or wget, and sha256sum (or shasum
# or openssl) to verify the download.
#
set -u

user=${USER:-$(id -un)}

usage() {
    echo "Usage: install.sh [--repo <owner/name>] [--version <tag|latest>]"
    echo "                   [--install-dir <dir>] [--hash <argon2-hash>]"
    echo "                   [--reset-password] [--bind-addr <addr:port>] [--no-start]"
    echo "                   [--uninstall [--purge]]"
    echo ""
    echo "  --repo <owner/name>  GitHub repo to fetch releases from (default: RazMag/sooth;"
    echo "                       env: SOOTH_REPO). Point this at a fork's own repo."
    echo "  --version <tag>      Release tag to install, e.g. v0.1.0 (default: latest;"
    echo "                       env: SOOTH_VERSION)."
    echo "  --install-dir <dir>  Where to put the sooth binary (default: ~/.local/bin;"
    echo "                       env: SOOTH_INSTALL_DIR)."
    echo "  --hash <hash>        Use this argon2 password hash instead of prompting"
    echo "                       (see: sooth --hash-password; env: SOOTH_HASH)."
    echo "  --reset-password     Prompt for a new password even if sooth.env already"
    echo "                       has one (env: SOOTH_RESET_PASSWORD=1)."
    echo "  --bind-addr <addr>   Written to sooth.env as SOOTH_BIND_ADDR (default: leave"
    echo "                       unset, i.e. sooth's own default 127.0.0.1:8420; env:"
    echo "                       SOOTH_BIND_ADDR)."
    echo "  --no-start           Install the binary and unit but skip enable/start (env:"
    echo "                       SOOTH_NO_START=1)."
    echo "  --uninstall          Stop, disable, and remove the service and binary, then"
    echo "                       exit (env: SOOTH_UNINSTALL=1). Keeps sooth.env (the"
    echo "                       password hash and config) unless --purge is also given."
    echo "  --purge              With --uninstall, also remove ~/.config/sooth (env:"
    echo "                       SOOTH_PURGE=1)."
}

repo_flag=${SOOTH_REPO:-}
version_flag=${SOOTH_VERSION:-}
install_dir_flag=${SOOTH_INSTALL_DIR:-}
hash_flag=${SOOTH_HASH:-}
reset_password_flag=${SOOTH_RESET_PASSWORD:-}
bind_addr_flag=${SOOTH_BIND_ADDR:-}
no_start_flag=${SOOTH_NO_START:-}
uninstall_flag=${SOOTH_UNINSTALL:-}
purge_flag=${SOOTH_PURGE:-}

while [ $# -gt 0 ]; do
    case "$1" in
        --repo)
            [ $# -ge 2 ] || { echo "error: $1 requires a value" >&2; exit 1; }
            repo_flag=$2
            shift 2
            ;;
        --repo=*)
            repo_flag=${1#--repo=}
            shift
            ;;
        --version)
            [ $# -ge 2 ] || { echo "error: $1 requires a value" >&2; exit 1; }
            version_flag=$2
            shift 2
            ;;
        --version=*)
            version_flag=${1#--version=}
            shift
            ;;
        --install-dir)
            [ $# -ge 2 ] || { echo "error: $1 requires a value" >&2; exit 1; }
            install_dir_flag=$2
            shift 2
            ;;
        --install-dir=*)
            install_dir_flag=${1#--install-dir=}
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
        --reset-password)
            reset_password_flag=1
            shift
            ;;
        --bind-addr)
            [ $# -ge 2 ] || { echo "error: $1 requires a value" >&2; exit 1; }
            bind_addr_flag=$2
            shift 2
            ;;
        --bind-addr=*)
            bind_addr_flag=${1#--bind-addr=}
            shift
            ;;
        --no-start)
            no_start_flag=1
            shift
            ;;
        --uninstall)
            uninstall_flag=1
            shift
            ;;
        --purge)
            purge_flag=1
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

repo=${repo_flag:-RazMag/sooth}
version=${version_flag:-latest}

if [ "$(id -u)" -eq 0 ]; then
    echo "error: sooth manages a user's own 'systemctl --user' session and must not be installed as root" >&2
    exit 1
fi

if ! command -v systemctl >/dev/null 2>&1; then
    echo "error: systemctl not found -- sooth requires a systemd user session" >&2
    exit 1
fi

if ! systemctl --user show-environment >/dev/null 2>&1; then
    echo "error: 'systemctl --user' isn't reachable. Run this from an active login" >&2
    echo "       session, or enable lingering first: loginctl enable-linger $user" >&2
    exit 1
fi

install_dir=${install_dir_flag:-$HOME/.local/bin}
bin=$install_dir/sooth
config_home=${XDG_CONFIG_HOME:-$HOME/.config}
sooth_dir=$config_home/sooth
env_file=$sooth_dir/sooth.env
unit_dir=$config_home/systemd/user
unit_file=$unit_dir/sooth.service

if [ -n "$uninstall_flag" ]; then
    echo "==> systemctl --user disable --now sooth.service"
    systemctl --user disable --now sooth.service 2>/dev/null || true

    if [ -f "$unit_file" ]; then
        echo "==> removing $unit_file"
        rm -f -- "$unit_file"
    fi
    systemctl --user daemon-reload || true
    systemctl --user reset-failed sooth.service 2>/dev/null || true

    if [ -x "$bin" ]; then
        echo "==> removing $bin"
        rm -f -- "$bin"
    fi

    if [ -n "$purge_flag" ]; then
        if [ -d "$sooth_dir" ]; then
            echo "==> removing $sooth_dir (--purge)"
            rm -rf -- "$sooth_dir"
        fi
    elif [ -d "$sooth_dir" ]; then
        echo "==> keeping $sooth_dir (password hash/config) -- rerun with --purge to remove it"
    fi

    echo "==> uninstalled"
    exit 0
fi

fetch() {
    # fetch <url> <output-path>
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        echo "error: need curl or wget to download sooth" >&2
        exit 1
    fi
}

case "$(uname -s)" in
    Linux) ;;
    *)
        echo "error: sooth only ships Linux releases (this is $(uname -s))" >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    x86_64|amd64) target=x86_64-unknown-linux-gnu ;;
    aarch64|arm64) target=aarch64-unknown-linux-gnu ;;
    *)
        echo "error: no sooth release for architecture $(uname -m) (only x86_64/aarch64 linux-gnu)" >&2
        exit 1
        ;;
esac

asset="sooth-$target"
if [ "$version" = latest ]; then
    base_url="https://github.com/$repo/releases/latest/download"
else
    base_url="https://github.com/$repo/releases/download/$version"
fi

work_dir=$(mktemp -d) || exit 1
trap 'rm -rf -- "$work_dir"' EXIT INT TERM

echo "==> downloading $asset ($version from $repo)"
fetch "$base_url/$asset" "$work_dir/sooth" || {
    echo "error: failed to download $base_url/$asset" >&2
    exit 1
}

echo "==> verifying checksum"
if fetch "$base_url/SHA256SUMS" "$work_dir/SHA256SUMS" 2>/dev/null && [ -s "$work_dir/SHA256SUMS" ]; then
    expected=$(grep -E "[[:space:]]\\*?$asset\$" "$work_dir/SHA256SUMS" | awk '{print $1}' | head -n1)
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$work_dir/sooth" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$work_dir/sooth" | awk '{print $1}')
    elif command -v openssl >/dev/null 2>&1; then
        actual=$(openssl dgst -sha256 "$work_dir/sooth" | awk '{print $NF}')
    else
        echo "warning: no sha256sum/shasum/openssl found, skipping checksum verification" >&2
        actual=
    fi
    if [ -n "$expected" ] && [ -n "$actual" ]; then
        if [ "$expected" != "$actual" ]; then
            echo "error: checksum mismatch for $asset (expected $expected, got $actual)" >&2
            exit 1
        fi
        echo "==> checksum OK"
    fi
else
    echo "warning: SHA256SUMS not available for this release, skipping checksum verification" >&2
fi

mkdir -p -- "$install_dir" || exit 1
chmod +x "$work_dir/sooth" || exit 1
mv -- "$work_dir/sooth" "$install_dir/sooth" || exit 1
echo "==> installed $bin"

case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "note: $install_dir isn't on PATH -- add it to your shell profile to run 'sooth' directly" ;;
esac

mkdir -p -- "$sooth_dir" "$unit_dir" || exit 1

existing_hash=
if [ -f "$env_file" ]; then
    existing_hash=$(sed -n 's/^SOOTH_AUTH_PASSWORD_HASH=//p' "$env_file" | tail -n1)
fi

if [ -n "$hash_flag" ]; then
    hash=$hash_flag
elif [ -n "$existing_hash" ] && [ -z "$reset_password_flag" ]; then
    echo "==> keeping existing password hash in $env_file (use --reset-password to change it)"
    hash=$existing_hash
else
    echo "==> no --hash given, generating one now"
    hash=$("$bin" --hash-password) || exit 1
fi

echo "==> writing $env_file"
umask 077
{
    echo "SOOTH_AUTH_PASSWORD_HASH=$hash"
    if [ -n "$bind_addr_flag" ]; then
        echo "SOOTH_BIND_ADDR=$bind_addr_flag"
    fi
} > "$env_file" || exit 1

echo "==> writing $unit_file"
cat > "$unit_file" <<EOF || exit 1
[Unit]
Description=sooth dashboard

[Service]
EnvironmentFile=$env_file
ExecStart=$bin
Restart=on-failure

[Install]
WantedBy=default.target
EOF

echo "==> systemctl --user daemon-reload"
systemctl --user daemon-reload || exit 1

if [ -n "$no_start_flag" ]; then
    echo "==> --no-start given, leaving sooth.service stopped"
    echo "    start it later with: systemctl --user enable --now sooth.service"
    exit 0
fi

echo "==> systemctl --user enable --now sooth.service"
systemctl --user enable --now sooth.service || exit 1
systemctl --user restart sooth.service || exit 1

echo "==> done. logs: journalctl --user -u sooth -f"
echo "    to survive logout/reboot without an active session, run:"
echo "      loginctl enable-linger $user"
