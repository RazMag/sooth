# Task runner for sooth. Thin wrappers around `cargo`, `npm`, and
# scripts/run-dev.sh -- see README.md#development for the full story.
#
# Install: https://github.com/casey/just
# List recipes: `just` or `just --list`

default:
    @just --list

# Build the debug binary (rebuilds frontend assets first if node_modules is present).
build:
    cargo build

# Build the release binary.
release:
    cargo build --release

# Build (if needed) and serve sooth directly, using real config/env discovery.
run:
    cargo run

# Run the unit tests.
test:
    cargo test

# Format the Rust source in place.
fmt:
    cargo fmt

# Fail if the Rust source isn't formatted (what CI runs).
fmt-check:
    cargo fmt --check

# Lint with clippy, denying warnings (what CI runs).
lint:
    cargo clippy --all-targets -- -D warnings

# fmt-check + lint + test, no frontend rebuild.
check: fmt-check lint test

# Install frontend deps and rebuild static/style.css + static/app.js.
frontend:
    npm ci
    npm run build

# Rebuild static/ on every frontend/** save (run in a second pane).
watch-frontend:
    npm run watch

# Build and run sooth against a scratch quadlet dir; forwards args, e.g. `just dev --port 8123 --fake-no-podman`.
dev *ARGS:
    scripts/run-dev.sh {{ARGS}}

# Mirrors .github/workflows/ci.yml locally: rebuild frontend (fail if static/ is stale), then fmt/clippy/test.
ci: frontend
    git diff --exit-code -- static/style.css static/app.js
    SOOTH_SKIP_FRONTEND_BUILD=1 cargo fmt --check
    SOOTH_SKIP_FRONTEND_BUILD=1 cargo clippy --all-targets -- -D warnings
    SOOTH_SKIP_FRONTEND_BUILD=1 cargo test
