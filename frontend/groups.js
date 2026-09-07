// Collapsible group sections in the list tables. Server renders every group
// header row (`tr.group-row[data-group]`) with its member rows
// (`tr[data-group-member]`) carrying `is-collapsed`, so the no-JS view starts
// fully collapsed. This module reconciles that against a per-browser set of
// *expanded* group paths in localStorage and wires the toggle buttons.

const KEY = "sooth:groups-expanded";

function loadExpanded() {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return new Set();
    const arr = JSON.parse(raw);
    return new Set(Array.isArray(arr) ? arr : []);
  } catch {
    return new Set();
  }
}

function saveExpanded(set) {
  try {
    localStorage.setItem(KEY, JSON.stringify([...set]));
  } catch {
    // private mode / disabled storage — collapse state just won't persist.
  }
}

// Every strict ancestor of `path` ("media/arr/x" -> ["media", "media/arr"]).
function ancestors(path) {
  const segs = path.split("/");
  const out = [];
  for (let i = 1; i < segs.length; i++) out.push(segs.slice(0, i).join("/"));
  return out;
}

function ancestorsExpanded(path, expanded) {
  return ancestors(path).every((a) => expanded.has(a));
}

function apply(root, expanded) {
  for (const hdr of root.querySelectorAll("tr.group-row")) {
    const path = hdr.dataset.group;
    hdr.classList.toggle("is-collapsed", !ancestorsExpanded(path, expanded));
    const btn = hdr.querySelector(".group-toggle");
    if (btn) btn.setAttribute("aria-expanded", expanded.has(path) ? "true" : "false");
  }
  for (const row of root.querySelectorAll("tr[data-group-member]")) {
    const path = row.dataset.groupMember;
    const visible = expanded.has(path) && ancestorsExpanded(path, expanded);
    row.classList.toggle("is-collapsed", !visible);
  }
}

export function initGroups() {
  const expanded = loadExpanded();
  for (const table of document.querySelectorAll(".data-table")) {
    apply(table, expanded);
  }

  if (initGroups._wired) return;
  initGroups._wired = true;

  document.addEventListener("click", (e) => {
    const btn = e.target.closest(".group-toggle");
    if (!btn) return;
    const hdr = btn.closest("tr.group-row");
    const table = btn.closest(".data-table");
    if (!hdr || !table) return;
    const path = hdr.dataset.group;
    const set = loadExpanded();
    if (set.has(path)) set.delete(path);
    else set.add(path);
    saveExpanded(set);
    apply(table, set);
  });
}
