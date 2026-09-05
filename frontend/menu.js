// Kebab menus. Each is a native `<details class="menu">`, but its `.menu-panel`
// lives inside a table wrapper that has `overflow-x: auto` -- and per the CSS
// spec that also clips the Y axis, so an absolutely-positioned panel would be
// cut off. On open we re-position the panel with `position: fixed` against the
// summary's rect so it escapes the clip; on close/scroll/outside-click we shut
// it. With JS off the panel still shows (just clipped) -- an acceptable
// fallback for a progressive-enhancement detail.
export function initMenus() {
  // `toggle` doesn't bubble -> listen in the capture phase.
  document.addEventListener(
    "toggle",
    (e) => {
      const menu = e.target;
      if (!menu.matches || !menu.matches("details.menu")) return;
      if (!menu.open) return;

      // Only one open at a time.
      for (const other of document.querySelectorAll("details.menu[open]")) {
        if (other !== menu) other.open = false;
      }

      const panel = menu.querySelector(".menu-panel");
      if (!panel) return;
      const r = menu.getBoundingClientRect();
      panel.style.position = "fixed";
      panel.style.top = `${Math.round(r.bottom + 4)}px`;
      panel.style.left = "auto";
      panel.style.right = `${Math.round(window.innerWidth - r.right)}px`;
    },
    true,
  );

  document.addEventListener("click", (e) => {
    for (const m of document.querySelectorAll("details.menu[open]")) {
      if (!m.contains(e.target)) m.open = false;
    }
  });

  window.addEventListener(
    "scroll",
    () => {
      for (const m of document.querySelectorAll("details.menu[open]")) m.open = false;
    },
    true,
  );
}
