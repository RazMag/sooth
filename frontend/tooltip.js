// Styled tooltips for `[data-tip]` triggers (the `templates::info_tip` icon).
// One shared `.tip` element, `position: fixed` against the trigger's rect --
// like the kebab menus (see menu.js), because the tables sit in an
// `overflow-x: auto` wrapper that would clip an absolutely-positioned popup.
// Shown on hover and on keyboard focus (and so on tap, which focuses the
// trigger); hidden on leave, blur, Escape, or any scroll. Listeners are
// delegated on `document`, so triggers swapped in by htmx need no re-init.
const GAP = 6;
// Keep the tooltip at least this far from the viewport edge.
const EDGE = 8;

export function initTooltips() {
  const tip = document.createElement("div");
  tip.className = "tip";
  tip.id = "sooth-tip";
  tip.setAttribute("role", "tooltip");
  tip.hidden = true;
  document.body.appendChild(tip);
  let current = null;

  const hide = () => {
    if (!current) return;
    current.removeAttribute("aria-describedby");
    current = null;
    tip.hidden = true;
  };

  const show = (el) => {
    if (current === el) return;
    hide();
    current = el;
    tip.textContent = el.dataset.tip;
    tip.hidden = false;
    el.setAttribute("aria-describedby", tip.id);
    const r = el.getBoundingClientRect();
    const t = tip.getBoundingClientRect();
    // Below the trigger unless it doesn't fit there and does above.
    let top = r.bottom + GAP;
    if (top + t.height > window.innerHeight - EDGE && r.top - GAP - t.height >= EDGE) {
      top = r.top - GAP - t.height;
    }
    const left = Math.min(
      Math.max(EDGE, r.left + r.width / 2 - t.width / 2),
      window.innerWidth - EDGE - t.width,
    );
    tip.style.top = `${top}px`;
    tip.style.left = `${left}px`;
  };

  const trigger = (e) => e.target.closest?.("[data-tip]");
  document.addEventListener("pointerover", (e) => {
    const el = trigger(e);
    if (el) show(el);
  });
  document.addEventListener("pointerout", (e) => {
    const el = trigger(e);
    if (el && el === current && !el.contains(e.relatedTarget) && document.activeElement !== el) {
      hide();
    }
  });
  document.addEventListener("focusin", (e) => {
    const el = trigger(e);
    if (el) show(el);
  });
  document.addEventListener("focusout", (e) => {
    if (trigger(e) === current) hide();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") hide();
  });
  window.addEventListener("scroll", hide, true);
  // The trigger may be swapped out from under an open tooltip.
  document.addEventListener("htmx:beforeSwap", hide);
}
