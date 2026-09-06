// The single bundled entry point (esbuild -> static/app.js). Pulls in htmx and
// its SSE extension, then the app's own small progressive-enhancement modules.

import htmx from "htmx.org";
import "htmx-ext-sse";

import { initTheme } from "./theme.js";
import { initFilter } from "./filter.js";
import { initNav } from "./nav.js";
import { initMenus } from "./menu.js";
import { initLogs } from "./logs.js";
import { initEditors } from "./editor.js";
import { initEnvVars } from "./envvars.js";

window.htmx = htmx;

// Delegated listeners (theme, filter, nav, menus) attach once; the
// DOM-scanning ones (logs, editors) are idempotent and re-run after htmx
// swaps in new content.
function boot() {
  initTheme();
  initFilter();
  initNav();
  initMenus();
  initLogs();
  initEditors();
  initEnvVars();
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", boot);
} else {
  boot();
}

document.addEventListener("htmx:afterSwap", () => {
  initLogs();
  initEditors();
  initEnvVars();
});
