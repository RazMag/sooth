// Progressively enhances the `.container` / `.build` editor's environment
// textarea (`<textarea name="env_vars" data-envvars-source>`, holding
// `KEY=VALUE` lines) into a list of Name/Value rows with add/remove buttons.
// The hidden textarea is kept in sync so a plain form POST still carries the
// same field -- and with JS off the textarea is just an editable KEY=VALUE
// box, which the server parses identically.

export function initEnvVars() {
  document.querySelectorAll("[data-envvars]").forEach((field) => {
    if (field.dataset.envvarsInit) return;
    field.dataset.envvarsInit = "1";

    const source = field.querySelector("[data-envvars-source]");
    if (!source) return;
    source.hidden = true;

    const rows = document.createElement("div");
    rows.className = "envvar-rows";
    source.after(rows);

    const addBtn = document.createElement("button");
    addBtn.type = "button";
    addBtn.className = "btn btn-ghost btn-sm";
    addBtn.textContent = "Add variable";
    rows.after(addBtn);

    const serialize = () => {
      const lines = [];
      rows.querySelectorAll(".envvar-row").forEach((row) => {
        const k = row.querySelector("[data-k]").value.trim();
        const v = row.querySelector("[data-v]").value;
        if (k) lines.push(k + "=" + v);
      });
      source.value = lines.join("\n");
    };

    const addRow = (k = "", v = "") => {
      const row = document.createElement("div");
      row.className = "envvar-row";

      const nameEl = document.createElement("input");
      nameEl.className = "input";
      nameEl.placeholder = "NAME";
      nameEl.setAttribute("data-k", "");
      nameEl.autocomplete = "off";
      nameEl.autocapitalize = "off";
      nameEl.spellcheck = false;
      nameEl.value = k;

      const valEl = document.createElement("input");
      valEl.className = "input";
      valEl.placeholder = "value";
      valEl.setAttribute("data-v", "");
      valEl.autocomplete = "off";
      valEl.spellcheck = false;
      valEl.value = v;

      const rm = document.createElement("button");
      rm.type = "button";
      rm.className = "btn btn-ghost btn-sm";
      rm.setAttribute("data-remove", "");
      rm.textContent = "Remove";
      rm.addEventListener("click", () => {
        row.remove();
        serialize();
      });

      row.append(nameEl, valEl, rm);
      rows.append(row);
    };

    // Parse existing KEY=VALUE lines into rows.
    const parsed = source.value
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => l && !l.startsWith("#") && !l.startsWith(";"));
    parsed.forEach((l) => {
      const i = l.indexOf("=");
      if (i === -1) return;
      addRow(l.slice(0, i).trim(), l.slice(i + 1));
    });
    // Start with one blank row so the editor is obviously usable.
    if (!rows.querySelector(".envvar-row")) addRow();

    rows.addEventListener("input", serialize);
    addBtn.addEventListener("click", () => {
      addRow();
      serialize();
    });
  });
}
