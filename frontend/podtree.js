// Collapsible pod rows in the Pods layout of the list tables. A pod row
// (`tr[data-pod]`) heads the rows of the units it owns or shares
// (`tr[data-pod-member]`, see `templates::list::PodTree`) -- as does the
// synthetic "Standalone" trunk (`data-pod=":standalone"`) for everything
// else; its chevron hides/shows them. Trunks start *expanded* -- the
// relation is the point -- so the server renders every child visible and
// this module only applies a per-browser set of *collapsed* pod keys from
// localStorage. Children are hidden with their own `pod-collapsed` class so
// they never fight `groups.js`'s `is-collapsed`.
//
// It also keeps each pod row's "N/M running" summary (`[data-pod-summary]`)
// current: member badges swap in place over SSE, which re-runs this module
// via `htmx:afterSwap`, so the count is re-read from the badges here rather
// than re-fetched.

const KEY = "sooth:pods-collapsed";

function loadCollapsed() {
  try {
    const arr = JSON.parse(localStorage.getItem(KEY) || "[]");
    return new Set(Array.isArray(arr) ? arr : []);
  } catch {
    return new Set();
  }
}

function saveCollapsed(set) {
  try {
    localStorage.setItem(KEY, JSON.stringify([...set]));
  } catch {
    // private mode / disabled storage — collapse state just won't persist.
  }
}

function refreshSummaries(root) {
  for (const summary of root.querySelectorAll("tr[data-pod] [data-pod-summary]")) {
    const pod = summary.closest("tr").dataset.pod;
    const members = [
      ...root.querySelectorAll(`tr[data-pod-member="${CSS.escape(pod)}"]`),
    ].filter((row) => row.querySelector(".kind-dot.kind-container"));
    if (members.length === 0) continue;
    const running = members.filter((row) => row.querySelector(".badge-running")).length;
    summary.textContent = `${running}/${members.length} running`;
  }
}

function apply(root, collapsed) {
  for (const row of root.querySelectorAll("tr[data-pod]")) {
    const btn = row.querySelector(".pod-toggle");
    if (btn) btn.setAttribute("aria-expanded", collapsed.has(row.dataset.pod) ? "false" : "true");
  }
  for (const row of root.querySelectorAll("tr[data-pod-member]")) {
    row.classList.toggle("pod-collapsed", collapsed.has(row.dataset.podMember));
  }
  refreshSummaries(root);
}

export function initPodTree() {
  const collapsed = loadCollapsed();
  for (const table of document.querySelectorAll(".data-table")) {
    apply(table, collapsed);
  }

  if (initPodTree._wired) return;
  initPodTree._wired = true;

  document.addEventListener("click", (e) => {
    const btn = e.target.closest(".pod-toggle");
    if (!btn) return;
    const row = btn.closest("tr[data-pod]");
    if (!row) return;
    const set = loadCollapsed();
    if (set.has(row.dataset.pod)) set.delete(row.dataset.pod);
    else set.add(row.dataset.pod);
    saveCollapsed(set);
    for (const table of document.querySelectorAll(".data-table")) apply(table, set);
  });
}
