//! Keeps the committed frontend artifacts (`static/style.css`,
//! `static/app.js`) in sync with `frontend/**` so `cargo build` / `cargo run`
//! pick up frontend edits automatically.
//!
//! Best-effort by design. `static/style.css` and `static/app.js` are checked
//! in, so a build with no Node toolchain (a fresh `cargo install`, CI without
//! a Node step, an offline build) still works -- it just uses whatever is
//! committed. Set `SOOTH_SKIP_FRONTEND_BUILD=1` to skip even when Node is
//! available (e.g. a read-only checkout).
//!
//! Thanks to the `rerun-if-changed` lines below, this only actually shells out
//! to npm when a frontend input has changed since the last build.

use std::path::Path;
use std::process::Command;

fn main() {
    for p in [
        "frontend",
        "package.json",
        "package-lock.json",
        "scripts/build-frontend.mjs",
    ] {
        println!("cargo:rerun-if-changed={p}");
    }
    println!("cargo:rerun-if-env-changed=SOOTH_SKIP_FRONTEND_BUILD");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let root = Path::new(&manifest_dir);
    let artifacts_exist =
        root.join("static/app.js").is_file() && root.join("static/style.css").is_file();

    if std::env::var_os("SOOTH_SKIP_FRONTEND_BUILD").is_some() {
        note("SOOTH_SKIP_FRONTEND_BUILD set -- using committed static/ assets");
        return;
    }

    if !root.join("node_modules").is_dir() {
        if artifacts_exist {
            note(
                "node_modules not found -- using committed static/ assets (run `npm ci && npm run build` to regenerate)",
            );
            return;
        }
        panic!(
            "frontend assets are missing and node_modules is not present.\nRun `npm ci && npm run build`, then rebuild."
        );
    }

    match Command::new("npm")
        .args(["run", "build"])
        .current_dir(root)
        .status()
    {
        Ok(s) if s.success() => {}
        Ok(s) => panic!("`npm run build` exited with {s}"),
        Err(e) if artifacts_exist => note(&format!(
            "could not run `npm run build` ({e}) -- using committed static/ assets"
        )),
        Err(e) => panic!(
            "could not run `npm run build` ({e}) and there are no committed static/ assets to fall back on"
        ),
    }
}

fn note(msg: &str) {
    println!("cargo:warning=frontend build: {msg}");
}
