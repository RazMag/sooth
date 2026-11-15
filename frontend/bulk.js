// The Services bulk bar (`templates::services::bulk_bar`): ticking a row's
// `.row-select` checkbox shows the bar with the count and fills its hidden
// `units` field; "Clear" or a finished action unticks everything. A
// `/rows` refresh replaces the checkboxes, so the bar re-counts after any
// htmx swap.
export function initBulk() {
  const sync = () => {
    const bar = document.getElementById("bulk-bar");
    if (!bar) return;
    const picked = [...document.querySelectorAll("input.row-select:checked")];
    bar.hidden = picked.length === 0;
    bar.querySelector("[data-bulk-count]").textContent = String(picked.length);
    bar.elements.units.value = picked.map((c) => c.value).join(",");
  };
  const clear = () => {
    for (const c of document.querySelectorAll("input.row-select:checked")) c.checked = false;
    sync();
  };

  document.addEventListener("change", (e) => {
    if (e.target.matches("input.row-select")) {
      const result = document.getElementById("bulk-result");
      if (result) result.textContent = "";
      sync();
    }
  });
  document.addEventListener("click", (e) => {
    if (e.target.closest("[data-bulk-clear]")) clear();
  });
  document.body.addEventListener("htmx:afterSwap", sync);
  // After an action, untick the rows but keep the bar up with its result.
  document.body.addEventListener("htmx:afterRequest", (e) => {
    if (e.target.id !== "bulk-bar" || !e.detail.successful) return;
    for (const c of document.querySelectorAll("input.row-select:checked")) c.checked = false;
    const bar = document.getElementById("bulk-bar");
    bar.elements.units.value = "";
    bar.querySelector("[data-bulk-count]").textContent = "0";
  });
}
