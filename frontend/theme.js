// Manual light/dark toggle. The pre-paint inline script in the page <head>
// applies the saved choice before first paint; this only flips + persists it.
export function initTheme() {
  document.addEventListener("click", (e) => {
    if (!e.target.closest("[data-theme-toggle]")) return;
    const root = document.documentElement;
    const current =
      root.dataset.theme ||
      (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
    const next = current === "dark" ? "light" : "dark";
    root.dataset.theme = next;
    try {
      localStorage.setItem("sooth-theme", next);
    } catch {
      /* private browsing etc. -- toggle still works for this load */
    }
  });
}
