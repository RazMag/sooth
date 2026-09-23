// Progressively enhances two things -- named for where the group picker
// first showed up (the "New Pod"/"Edit Pod" pages), but `initPodGroupField`
// itself scans for any `[data-group-field]` on the page, so it applies
// equally to the Git Sync "Add" form's group field:
//
// - Group: a plain text input + <datalist> (the no-JS fallback) becomes the
//   same click-a-group-or-type-a-new-one disclosure the detail page's group
//   control uses.
// - Containers/Networks/Volumes (pods only): each is an inline picker (a
//   native <details> disclosure, part of the page's own flow -- no popup)
//   for attaching already-defined quadlets, one `[data-resource-field]` per
//   kind. All three share the same generic logic here; a volume's row
//   additionally carries a destination-path input (`[data-resource-dest]`),
//   which is the only thing that changes how a selection serializes.
//
// Every field keeps a hidden textarea in sync so a plain form POST still
// carries the same value, and with JS off it's just a plain editable input
// the server parses the same way -- same shape as envvars.js.

export function initPodGroupField() {
  document.querySelectorAll("[data-group-field]").forEach((field) => {
    if (field.dataset.groupFieldInit) return;
    field.dataset.groupFieldInit = "1";

    const input = field.querySelector("[data-group-source]");
    const picker = field.querySelector("[data-group-picker]");
    if (!input || !picker) return;

    input.hidden = true;
    picker.hidden = false;

    const labelEl = picker.querySelector("[data-group-picker-label]");
    const emptyLabel = labelEl.dataset.emptyLabel || "root";
    const summary = picker.querySelector("summary");

    const setGroup = (value) => {
      input.value = value;
      labelEl.textContent = value === "" ? emptyLabel : value;
      picker.open = false;
      input.setCustomValidity("");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    };

    picker.querySelectorAll("[data-group-choice]").forEach((btn) => {
      btn.addEventListener("click", () => setGroup(btn.dataset.groupChoice));
    });

    const newInput = picker.querySelector("[data-group-new-input]");

    // The source input is hidden, so a `required` one left empty can't be
    // focused for the browser's own validation bubble (Firefox logs "The
    // invalid form control ... is not focusable" and the submit just does
    // nothing). Take over: cancel the native report and open the picker
    // with its new-group field focused instead.
    input.addEventListener("invalid", (e) => {
      e.preventDefault();
      picker.open = true;
      if (summary) summary.classList.add("group-picker-invalid");
      if (newInput) {
        newInput.focus();
      } else if (summary) {
        summary.focus();
      }
    });
    input.addEventListener("input", () => {
      if (summary) summary.classList.remove("group-picker-invalid");
    });

    const addBtn = picker.querySelector("[data-group-new-add]");
    const addNew = () => {
      const v = newInput.value.trim();
      if (!v) return;
      setGroup(v);
      newInput.value = "";
    };
    if (addBtn) addBtn.addEventListener("click", addNew);
    if (newInput) {
      newInput.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          addNew();
        }
      });
    }
  });
}

export function initPodResourcePickers() {
  document.querySelectorAll("[data-resource-field]").forEach((field) => {
    if (field.dataset.resourceFieldInit) return;
    field.dataset.resourceFieldInit = "1";
    initResourcePicker(field);
  });
}

function initResourcePicker(field) {
  const source = field.querySelector("[data-resource-source]");
  const chips = field.querySelector("[data-resource-chips]");
  const panel = field.querySelector("[data-resource-picker]");
  if (!source || !chips || !panel) return;

  source.hidden = true;

  const rows = () => Array.from(panel.querySelectorAll("[data-resource-row]"));
  const boxIn = (row) => row.querySelector('input[type="checkbox"]');
  const destIn = (row) => row.querySelector("[data-resource-dest]");
  const labelOf = (row) => row.querySelector(".pod-picker-name").textContent;
  const rowFor = (fileName) => rows().find((r) => boxIn(r).value === fileName);

  // file name -> { label, dest } (dest only present for rows that carry a
  // destination input, i.e. volumes); insertion order preserved for chips.
  const selected = new Map();

  const serialize = () => {
    source.value = Array.from(selected.entries())
      .map(([fileName, entry]) =>
        entry.dest === undefined ? fileName : `${fileName}=${entry.dest}`,
      )
      .join("\n");
  };

  const renderChips = () => {
    chips.innerHTML = "";
    selected.forEach((entry, fileName) => {
      const chip = document.createElement("span");
      chip.className = "chip staged-chip";
      const text = entry.dest ? `${entry.label} → ${entry.dest}` : entry.label;
      chip.append(document.createTextNode(text));

      const rm = document.createElement("button");
      rm.type = "button";
      rm.className = "staged-chip-remove";
      rm.setAttribute("aria-label", "Remove " + fileName);
      rm.textContent = "×";
      rm.addEventListener("click", () => {
        selected.delete(fileName);
        const row = rowFor(fileName);
        if (row) boxIn(row).checked = false;
        renderChips();
        serialize();
      });

      chip.append(rm);
      chips.append(chip);
    });
  };

  const stage = (row) => {
    const box = boxIn(row);
    const dest = destIn(row);
    if (box.checked) {
      selected.set(box.value, {
        label: labelOf(row),
        dest: dest ? dest.value.trim() : undefined,
      });
    } else {
      selected.delete(box.value);
    }
    renderChips();
    serialize();
  };

  // Seed from a pre-filled textarea (a 422 redisplay carries the staged list
  // back as plain text, same as the env-var editor does).
  source.value
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean)
    .forEach((line) => {
      const eq = line.indexOf("=");
      const fileName = eq === -1 ? line : line.slice(0, eq);
      const dest = eq === -1 ? undefined : line.slice(eq + 1);
      const row = rowFor(fileName);
      selected.set(fileName, { label: row ? labelOf(row) : fileName, dest });
      if (row) {
        boxIn(row).checked = true;
        if (dest !== undefined && destIn(row)) destIn(row).value = dest;
      }
    });
  renderChips();
  serialize();

  panel.addEventListener("change", (e) => {
    if (e.target.matches('input[type="checkbox"]')) {
      stage(e.target.closest("[data-resource-row]"));
    }
  });
  panel.addEventListener("input", (e) => {
    if (!e.target.matches("[data-resource-dest]")) return;
    const row = e.target.closest("[data-resource-row]");
    const box = boxIn(row);
    if (e.target.value.trim() !== "" && !box.checked) box.checked = true;
    if (box.checked) stage(row);
  });

  const filterInput = panel.querySelector("[data-resource-filter]");
  if (filterInput) {
    filterInput.addEventListener("input", () => {
      const needle = filterInput.value.trim().toLowerCase();
      rows().forEach((row) => {
        const haystack = (row.dataset.resourceRow || "").toLowerCase();
        row.hidden = needle.length > 0 && !haystack.includes(needle);
      });
    });
  }
}
