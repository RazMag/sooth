// Live client-side row filter for list tables. An
// `<input data-filter-target="rows-id">` hides non-matching rows in the
// `<tbody id="rows-id">`.
export function initFilter() {
  document.addEventListener("input", (e) => {
    const input = e.target.closest("[data-filter-target]");
    if (!input) return;
    const target = document.getElementById(input.dataset.filterTarget);
    if (!target) return;
    const needle = input.value.trim().toLowerCase();
    for (const row of target.querySelectorAll(":scope > tr")) {
      row.hidden = needle.length > 0 && !row.textContent.toLowerCase().includes(needle);
    }
  });
}
