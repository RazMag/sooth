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
      if (el.name && el.name in values) el.value = values[el.name];
    }
  }
}

export function initGitSync() {
  const container = document.getElementById("git-sync-rows");
  if (!container || initGitSync._wired === container) return;
  initGitSync._wired = container;

  let pending = null;
  container.addEventListener("htmx:beforeSwap", () => {
    pending = captureOpenDisclosures(container);
  });
  container.addEventListener("htmx:afterSwap", () => {
    if (pending) restoreOpenDisclosures(container, pending);
    pending = null;
  });
}
