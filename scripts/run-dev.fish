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

argparse 'd/dir=' 'p/port=' 'hash=' 'release' 'no-seed' 'h/help' -- $argv
or exit 1

if set -q _flag_help
    echo "Usage: run-dev.fish [--dir <path>] [--port <n>] [--hash <argon2-hash>] [--release] [--no-seed]"
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

echo "==> starting sooth on http://127.0.0.1:$port  (quadlet dir: $scratch)"
env SOOTH_QUADLET_DIR=$scratch \
    SOOTH_AUTH_PASSWORD_HASH=$hash \
    SOOTH_BIND_ADDR=127.0.0.1:$port \
    $bin
