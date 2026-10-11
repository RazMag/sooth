// Kebab menus. Each is a native `<details class="menu">`, but its `.menu-panel`
// lives inside a table wrapper that has `overflow-x: auto` -- and per the CSS
// spec that also clips the Y axis, so an absolutely-positioned panel would be
// cut off. On open we re-position the panel with `position: fixed` against the
// summary's rect so it escapes the clip; while it's open we keep it glued to
// the summary on scroll/resize (rather than closing on any scroll -- a
// layout-shifting htmx swap elsewhere on the page can nudge the scroll
// position, and that must not slam the menu shut). Close on outside-click,
// Escape, or once the summary scrolls out of view. With JS off the panel
// still shows (just clipped) -- an acceptable progressive-enhancement fallback.
//
// A panel opens below its summary unless it doesn't fit there and there's
// more room above (a row near the bottom of a long table), in which case it
// opens upward; if it fits neither way it's capped to the larger side and
// scrolls. The row kebab's nested "move to group" picker flips the same way.
// Content swapped into an open panel (the update check, the live status
// forms) re-runs the placement, since the panel's height changed.
const GAP = 4;
// Keep panels at least this far from the viewport edge.
const EDGE = 8;

export function initMenus() {
  const placePicker = (menu) => {
    const panel = menu.querySelector("details.group-picker[open] > .group-picker-panel");
    if (!panel) return;
    panel.style.top = "";
    panel.style.bottom = "";
    const r = panel.getBoundingClientRect();
    const summary = panel.parentElement.querySelector("summary");
    const s = summary ? summary.getBoundingClientRect() : r;
    const fitsBelow = r.bottom <= window.innerHeight - EDGE;
    if (!fitsBelow && s.top - r.height - GAP >= EDGE) {
      panel.style.top = "auto";
      panel.style.bottom = `calc(100% + ${GAP}px)`;
    }
  };

  const positionPanel = (menu) => {
    const panel = menu.querySelector(".menu-panel");
    if (!panel) return;
    const r = menu.getBoundingClientRect();
    panel.style.position = "fixed";
    panel.style.left = "auto";
    // Right-aligned to the summary, but never past the viewport's right edge
    // (the summary itself can be scrolled off it in a wide table).
    panel.style.right = `${Math.round(Math.max(window.innerWidth - r.right, EDGE))}px`;
    // Measure at natural height before choosing a side.
    panel.style.maxHeight = "";
    panel.style.overflowY = "";
    const height = panel.offsetHeight;
    const below = window.innerHeight - r.bottom - GAP - EDGE;
    const above = r.top - GAP - EDGE;
    const up = height > below && above > below;
    if (up) {
      panel.style.top = "auto";
      panel.style.bottom = `${Math.round(window.innerHeight - r.top + GAP)}px`;
    } else {
      panel.style.top = `${Math.round(r.bottom + GAP)}px`;
      panel.style.bottom = "auto";
    }
    const room = Math.max(up ? above : below, 0);
    if (height > room) {
      panel.style.maxHeight = `${Math.floor(room)}px`;
      panel.style.overflowY = "auto";
    }
    placePicker(menu);
  };

  // `toggle` doesn't bubble -> listen in the capture phase.
  document.addEventListener(
    "toggle",
    (e) => {
      const el = e.target;
      if (!el.matches) return;
      // The kebab's nested group picker opening or closing changes what
      // needs to fit.
      if (el.matches("details.group-picker")) {
        const menu = el.closest("details.menu[open]");
        if (menu) positionPanel(menu);
        return;
      }
      const menu = el;
      if (!menu.matches("details.menu") || !menu.open) return;
      // Only one open at a time.
      for (const other of document.querySelectorAll("details.menu[open]")) {
        if (other !== menu) other.open = false;
      }
      positionPanel(menu);
    },
    true,
  );

  document.addEventListener("click", (e) => {
    for (const m of document.querySelectorAll("details.menu[open]")) {
      if (!m.contains(e.target)) m.open = false;
    }
  });

  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    for (const m of document.querySelectorAll("details.menu[open]")) m.open = false;
  });

  let ticking = false;
  const reflow = () => {
    if (ticking) return;
    ticking = true;
    requestAnimationFrame(() => {
      ticking = false;
      for (const m of document.querySelectorAll("details.menu[open]")) {
        const r = m.getBoundingClientRect();
        const onScreen = r.bottom > 0 && r.top < window.innerHeight;
        if (onScreen) positionPanel(m);
        else m.open = false;
      }
    });
  };
  window.addEventListener("scroll", reflow, true);
  window.addEventListener("resize", reflow);
  document.addEventListener("htmx:afterSettle", reflow);
}
