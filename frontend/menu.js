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
export function initMenus() {
  const positionPanel = (menu) => {
    const panel = menu.querySelector(".menu-panel");
    if (!panel) return;
    const r = menu.getBoundingClientRect();
    panel.style.position = "fixed";
    panel.style.top = `${Math.round(r.bottom + 4)}px`;
    panel.style.left = "auto";
    panel.style.right = `${Math.round(window.innerWidth - r.right)}px`;
  };

  // `toggle` doesn't bubble -> listen in the capture phase.
  document.addEventListener(
    "toggle",
    (e) => {
      const menu = e.target;
      if (!menu.matches || !menu.matches("details.menu") || !menu.open) return;
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
}
