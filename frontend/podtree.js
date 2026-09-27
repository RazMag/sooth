// Collapsible pod rows in the list tables. A pod row (`tr[data-pod]`) heads
// the rows of the units it owns (`tr[data-pod-member]`, see
// `templates::list::PodTree`); its chevron hides/shows them. Unlike groups,
// pods start *expanded* -- the relation is the point -- so the server renders
// every child visible and this module only applies a per-browser set of
// *collapsed* pod file names from localStorage. Children are hidden with
// their own `pod-collapsed` class so a collapsed group (`is-collapsed`,
// groups.js) and a collapsed pod never fight over one class.

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

function apply(root, collapsed) {
  for (const row of root.querySelectorAll("tr[data-pod]")) {
    const btn = row.querySelector(".pod-toggle");
    if (btn) btn.setAttribute("aria-expanded", collapsed.has(row.dataset.pod) ? "false" : "true");
  }
  for (const row of root.querySelectorAll("tr[data-pod-member]")) {
    row.classList.toggle("pod-collapsed", collapsed.has(row.dataset.podMember));
  }
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
