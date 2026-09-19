// Light/dark/system theme preference. The pre-paint inline script in the
// page <head> resolves the saved preference into an effective light/dark
// theme and stamps both on <html> before first paint; this module only
// cycles + persists the preference, and keeps the effective theme in sync
// while it's "system" (an OS-level scheme change fires while the page is
// still open).
const STORAGE_KEY = "sooth-theme";

function effectiveTheme(pref) {
  if (pref === "light" || pref === "dark") return pref;
  return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyPref(pref) {
  const root = document.documentElement;
  root.dataset.theme = effectiveTheme(pref);
  root.dataset.themePref = pref;
}

export function initTheme() {
  document.addEventListener("click", (e) => {
    if (!e.target.closest("[data-theme-toggle]")) return;
    const current = document.documentElement.dataset.themePref || "system";
    const next = current === "light" ? "dark" : current === "dark" ? "system" : "light";
    applyPref(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      /* private browsing etc. -- toggle still works for this load */
    }
  });

  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
    if (document.documentElement.dataset.themePref === "system") applyPref("system");
  });
}
