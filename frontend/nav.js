// Small-screen sidebar toggle. Flips `data-nav-open` on `.app-shell`; closes
// on scrim click or Escape. At wide viewports the CSS ignores all of this and
// the sidebar is always visible.
export function initNav() {
  const shell = document.querySelector(".app-shell");
  if (!shell) return;
  const close = () => shell.removeAttribute("data-nav-open");

  document.addEventListener("click", (e) => {
    if (e.target.closest("[data-nav-toggle]")) {
      shell.toggleAttribute("data-nav-open");
    } else if (e.target.closest("[data-nav-scrim]")) {
      close();
    }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") close();
  });
  // A nav link click on mobile should dismiss the drawer.
  shell.querySelector(".sidebar")?.addEventListener("click", (e) => {
    if (e.target.closest("a")) close();
  });
}
