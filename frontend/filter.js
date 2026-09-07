// Live client-side row filter for list tables. An
// `<input data-filter-target="rows-id">` hides non-matching rows in the
// `<tbody id="rows-id">`. Group header rows (`tr.group-row`) are never hidden
// by the needle themselves; while a needle is active the tbody gets
// `data-filtering`, which the CSS uses to reveal rows inside collapsed groups
// so a match is never buried.
export function initFilter() {
  document.addEventListener("input", (e) => {
    const input = e.target.closest("[data-filter-target]");
    if (!input) return;
    const target = document.getElementById(input.dataset.filterTarget);
    if (!target) return;
    const needle = input.value.trim().toLowerCase();

    target.toggleAttribute("data-filtering", needle.length > 0);

    const rows = target.querySelectorAll(":scope > tr");
    for (const row of rows) {
      if (row.classList.contains("group-row")) continue;
      row.hidden = needle.length > 0 && !row.textContent.toLowerCase().includes(needle);
    }
    // A group header with no visible members is just noise while filtering.
    for (const hdr of target.querySelectorAll(":scope > tr.group-row")) {
      if (needle.length === 0) {
        hdr.hidden = false;
        continue;
      }
      const path = hdr.dataset.group;
      let anyVisible = false;
      for (const row of target.querySelectorAll(
        `:scope > tr[data-group-member="${CSS.escape(path)}"]`,
      )) {
        if (!row.hidden) {
          anyVisible = true;
          break;
        }
      }
      hdr.hidden = !anyVisible;
    }
  });
}
