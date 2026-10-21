// The Git Sync page's `#git-sync-rows` container refreshes on every
// `sse:git-sync-changed` tick -- which fires on routine progress too
// (Checking -> UpToDate), not just on add/remove/edit -- by re-rendering
// *every* card, since a status change on one sync isn't otherwise isolated
// from the others' markup. Left alone, that `hx-swap="innerHTML"` would
// silently close whichever card's Edit/Remove `<details>` the user had
// open (and discard anything they'd started typing into it) the moment a
// poll landed mid-interaction -- the same class of problem the unit list's
// tbody avoids by never doing a wholesale swap on a status tick (see
// templates/list.rs). Reopening the matching disclosure and restoring its
// field values across the swap sidesteps that without having to split the
// card into a bunch of separately-swapped fragments.

function captureOpenDisclosures(container) {
  const open = [];
  for (const details of container.querySelectorAll("details.gitsync-disclosure[open]")) {
    const values = {};
    for (const el of details.querySelectorAll("input, textarea, select")) {
      if (el.name) values[el.name] = el.value;
    }
    open.push({ group: details.dataset.group, kind: details.dataset.kind, values });
  }
  return open;
}

function restoreOpenDisclosures(container, open) {
  for (const { group, kind, values } of open) {
    const details = [...container.querySelectorAll("details.gitsync-disclosure")].find(
      (d) => d.dataset.group === group && d.dataset.kind === kind
    );
    if (!details) continue;
    details.open = true;
    for (const el of details.querySelectorAll("input, textarea, select")) {
      if (!el.name || !(el.name in values)) continue;
      const value = values[el.name];
      // A fresh branch <select> holds only the configured branch until its
      // listing re-loads (which posts the form, so it keeps this choice).
      if (el.tagName === "SELECT" && ![...el.options].some((o) => o.value === value)) {
        el.add(new Option(value, value));
      }
      el.value = value;
    }
  }
}

export function initGitSync() {
  const container = document.getElementById("git-sync-rows");
  if (!container || initGitSync._wired === container) return;
  initGitSync._wired = container;

  // An Edit form lists its remote's branches only once it's opened (see
  // `branch_field` in templates/gitsync.rs), not for every card on every
  // refresh. `toggle` doesn't bubble, hence the capture listener; a reopen
  // by `restoreOpenDisclosures` fires it too, re-listing the fresh form --
  // before htmx has processed the swapped-in picker (it does at settle),
  // hence the (idempotent) `process` first.
  container.addEventListener(
    "toggle",
    (e) => {
      const picker = e.target.open && e.target.querySelector?.(".branch-picker");
      if (!picker) return;
      window.htmx.process(picker);
      window.htmx.trigger(picker, "gitsync:list-branches");
    },
    true
  );

  let pending = null;
  // Only the container's own refresh -- a branch picker's swap inside a
  // card bubbles up here too.
  container.addEventListener("htmx:beforeSwap", (e) => {
    if (e.detail.target === container) pending = captureOpenDisclosures(container);
  });
  container.addEventListener("htmx:afterSwap", (e) => {
    if (e.detail.target !== container) return;
    if (pending) restoreOpenDisclosures(container, pending);
    pending = null;
  });
}
