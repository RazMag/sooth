// Live client-side row filter for list tables. An
// `<input data-filter-target="rows-id">` hides non-matching rows in the
// `<tbody id="rows-id">`. Group header rows (`tr.group-row`) are never hidden
// by the needle themselves; while a needle is active the tbody gets
// `data-filtering`, which the CSS uses to reveal rows inside collapsed groups
// and pods so a match is never buried. A pod row stays visible while any of
// its units matches, so a match keeps its context.
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
    for (const pod of target.querySelectorAll(":scope > tr[data-pod][hidden]")) {
      const kids = target.querySelectorAll(
        `:scope > tr[data-pod-member="${CSS.escape(pod.dataset.pod)}"]`,
      );
      pod.hidden = ![...kids].some((row) => !row.hidden);
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
