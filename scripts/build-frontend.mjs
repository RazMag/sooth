// Builds the two committed frontend artifacts consumed by `rust-embed`:
//
//   frontend/styles.css  --(Tailwind v4)-->  static/style.css
//   frontend/main.js      --(esbuild)------>  static/app.js  (+ .map)
//
// `cargo build` does NOT run this -- the outputs are checked in, like the
// vendored blobs used to be. Run `npm run build` after touching frontend/**,
// or `npm run watch` while working on it. CI rebuilds and diffs static/.

import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import path from "node:path";
import * as esbuild from "esbuild";

const execFileP = promisify(execFile);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const watch = process.argv.includes("--watch");

const tailwindBin = path.join(root, "node_modules", "@tailwindcss", "cli", "dist", "index.mjs");
const cssIn = path.join(root, "frontend", "styles.css");
const cssOut = path.join(root, "static", "style.css");
const jsIn = path.join(root, "frontend", "main.js");
const jsOut = path.join(root, "static", "app.js");

/** @type {import("esbuild").BuildOptions} */
const esbuildOptions = {
  entryPoints: [jsIn],
  outfile: jsOut,
  bundle: true,
  format: "iife",
  target: "es2020",
  minify: !watch,
  sourcemap: true,
  legalComments: "none",
  logLevel: "info",
};

function tailwind(extraArgs) {
  const args = [tailwindBin, "-i", cssIn, "-o", cssOut, ...extraArgs];
  return execFile(process.execPath, args, { cwd: root });
}

if (watch) {
  const ctx = await esbuild.context(esbuildOptions);
  await ctx.watch();
  const cp = tailwind(["--watch"]);
  cp.stdout?.pipe(process.stdout);
  cp.stderr?.pipe(process.stderr);
  console.log("watching frontend/ -> static/ (Ctrl-C to stop)");
} else {
  await Promise.all([
    promisify((cb) => {
      const cp = tailwind(["--minify"]);
      cp.on("error", cb);
      cp.on("exit", (code) => cb(code ? new Error(`tailwind exited ${code}`) : null));
    })(),
    esbuild.build(esbuildOptions),
  ]);
  console.log("frontend build complete");
}
