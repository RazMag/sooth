// The Git Sync page's `#git-sync-rows` container refreshes on every
// `sse:git-sync-changed` tick -- which fires on routine progress too
// (Checking -> UpToDate), not just on add/remove/edit -- by re-rendering
// *every* card, since a status change on one sync isn't otherwise isolated
// from the others' markup. Left alone, that `hx-swap="innerHTML"` would
// silently close whichever card's Remove `<details>` the user had
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
  wireDialogs();
  // A rejected Add re-renders the page with its dialog flagged to reopen;
  // drop the flag so a later afterSwap re-run doesn't reopen it again.
  const reopen = document.querySelector("dialog[data-gitsync-open-on-load]");
  if (reopen) {
    reopen.removeAttribute("data-gitsync-open-on-load");
    reopen.showModal();
  }

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

// The Add and Edit forms are <dialog>s outside `#git-sync-rows` (see
// templates/gitsync.rs), so a refresh never touches them: Add is one plain
// form post, Edit one shared htmx form filled from the clicked card's Edit
// button -- the same shape as the Secrets page's Set/Replace dialog.
function wireDialogs() {
  if (window.__soothGitSyncWired) return;
  window.__soothGitSyncWired = true;

  document.addEventListener("click", (e) => {
    if (e.target.closest("[data-gitsync-add]")) return void openAdd();

    const edit = e.target.closest("[data-gitsync-edit]");
    if (edit) return void openEdit(edit);

    const close = e.target.closest("[data-gitsync-dialog-close]");
    if (close) return void close.closest("dialog").close();

    // A click on the backdrop targets the dialog itself but lands outside
    // its box (same technique as secrets.js).
    const dialog = e.target.closest("[data-gitsync-dialog]");
    if (dialog && e.target === dialog) {
      const r = dialog.getBoundingClientRect();
      const inside =
        e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
      if (!inside) dialog.close();
    }
  });

  // The Add post blocks on a real `git clone`, a genuine multi-second
  // wait: show it as working, not stalled.
  document.addEventListener("submit", (e) => {
    if (e.target.id !== "git-sync-add-form") return;
    const submit = document.getElementById("git-sync-add-submit");
    submit.classList.add("btn-loading");
    submit.disabled = true;
    document.getElementById("git-sync-add-wait").hidden = false;
  });

  // Edit: close on success (the `git-sync-changed` refresh redraws the
  // card); otherwise show the server's error fragment in the dialog. The
  // branch picker's own requests bubble here too -- only the form counts.
  document.addEventListener("htmx:afterRequest", (e) => {
    const form = e.detail.elt;
    const dialog = form.tagName === "FORM" && form.closest("[data-gitsync-edit-dialog]");
    if (!dialog) return;
    if (e.detail.successful) return void dialog.close();
    const slot = dialog.querySelector("[data-gitsync-edit-error]");
    if (e.detail.xhr.responseText) slot.innerHTML = e.detail.xhr.responseText;
    else slot.textContent = "The change didn't go through — check the remote and try again.";
  });
}

function openAdd() {
  const dialog = document.getElementById("git-sync-add-dialog");
  if (!dialog) return;
  dialog.showModal();
  document.getElementById("remote")?.focus();
}

function openEdit(btn) {
  const dialog = document.querySelector("[data-gitsync-edit-dialog]");
  if (!dialog) return;
  const form = dialog.querySelector("form");
  const { gitsyncEdit: group, remote, branch, interval } = btn.dataset;

  dialog.querySelector("[data-gitsync-edit-group]").textContent = group;
  dialog.querySelector("[data-gitsync-edit-error]").replaceChildren();
  form.elements.group.value = group;
  form.elements.remote.value = remote;
  form.elements.poll_interval_secs.value = interval;

  // Start from just the tracked branch; the listing swaps in the rest (and
  // posts the form, so it keeps this one selected).
  const picker = form.querySelector(".branch-picker");
  const select = document.createElement("select");
  select.className = "input";
  select.id = "git-sync-edit-branch";
  select.name = "branch";
  select.add(new Option(branch, branch, true, true));
  picker.replaceChildren(select);

  dialog.showModal();
  window.htmx.trigger(picker, "gitsync:list-branches");
}
