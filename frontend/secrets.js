// Secrets page: the Value column's eye toggle (one button that shows and
// hides), its copy button, and the shared Set/Replace <dialog>.
//
// A value is fetched only when the eye is clicked -- a CSRF-checked POST to
// `/secrets/reveal`, answered as `no-store` plain text -- and is set as
// `textContent`, never parsed as markup. Hiding just clears it again; there's
// nothing to tell the server.

export function initSecrets() {
  if (window.__soothSecretsWired) return;
  window.__soothSecretsWired = true;

  document.addEventListener("click", (e) => {
    const toggle = e.target.closest("[data-secret-toggle]");
    if (toggle) return void onToggle(toggle);

    const copy = e.target.closest("[data-secret-copy]");
    if (copy) return void onCopy(copy);

    const edit = e.target.closest("[data-secret-edit]");
    if (edit) return void openDialog(edit);

    const close = e.target.closest("[data-secret-dialog-close]");
    if (close) return void close.closest("dialog").close();

    // A click on the backdrop targets the dialog itself but lands outside
    // its box (same technique as podmembers.js).
    const dialog = e.target.closest("[data-secret-dialog]");
    if (dialog && e.target === dialog) {
      const r = dialog.getBoundingClientRect();
      const inside =
        e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
      if (!inside) dialog.close();
    }
  });
}

function setShown(line, value) {
  const shown = value !== null;
  const plain = line.querySelector(".secret-plain");
  const toggle = line.querySelector("[data-secret-toggle]");
  const copy = line.querySelector("[data-secret-copy]");
  const name = line.dataset.secret;

  plain.textContent = shown ? value : "";
  plain.title = shown ? value : "";
  plain.hidden = !shown;
  line.querySelector(".secret-mask").hidden = shown;
  toggle.querySelector(".icon-when-hidden").hidden = shown;
  toggle.querySelector(".icon-when-shown").hidden = !shown;
  toggle.setAttribute("aria-pressed", String(shown));
  toggle.setAttribute("aria-label", (shown ? "Hide value of " : "Show value of ") + name);
  toggle.title = shown ? "Hide value" : "Show value";
  // Toggle visibility, not `hidden`: the button keeps its space either way,
  // so revealing a value doesn't reflow the table.
  if (copy) {
    copy.classList.toggle("secret-copy-off", !shown);
    copy.tabIndex = shown ? 0 : -1;
    copy.setAttribute("aria-hidden", String(!shown));
  }
}

async function onToggle(toggle) {
  const line = toggle.closest("[data-secret]");
  if (toggle.getAttribute("aria-pressed") === "true") {
    setShown(line, null);
    return;
  }
  const table = toggle.closest("[data-secrets-csrf]");
  toggle.disabled = true;
  try {
    const res = await fetch("/secrets/reveal", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        csrf_token: table ? table.dataset.secretsCsrf : "",
        name: line.dataset.secret,
      }),
      cache: "no-store",
    });
    if (!res.ok) throw new Error(String(res.status));
    setShown(line, await res.text());
  } catch {
    toggle.title = "Could not load the value — reload the page and try again";
  } finally {
    toggle.disabled = false;
  }
}

async function onCopy(copy) {
  const plain = copy.closest("[data-secret]").querySelector(".secret-plain");
  if (!navigator.clipboard) return;
  try {
    await navigator.clipboard.writeText(plain.textContent);
  } catch {
    return;
  }
  const idle = copy.querySelector(".icon-copy");
  const done = copy.querySelector(".icon-copied");
  idle.hidden = true;
  done.hidden = false;
  setTimeout(() => {
    idle.hidden = false;
    done.hidden = true;
  }, 1500);
}

function openDialog(btn) {
  const dialog = document.querySelector("[data-secret-dialog]");
  if (!dialog) return;
  const form = dialog.querySelector("form");
  const name = btn.dataset.secretEdit;
  const replace = btn.dataset.mode === "replace";
  const users = Number(btn.dataset.users || 0);

  dialog.querySelector("[data-secret-dialog-title]").textContent = replace ? "Replace" : "Set";
  dialog.querySelector("[data-secret-dialog-name]").textContent = name;
  form.elements.name.value = name;
  form.elements.replace.disabled = !replace;
  form.elements.value.value = "";

  const restartField = dialog.querySelector("[data-secret-dialog-restart]");
  restartField.hidden = users === 0;
  form.elements.restart.disabled = users === 0;
  form.elements.restart.checked = true;
  const units = users === 1 ? "1 unit" : `${users} units`;
  dialog.querySelector("[data-secret-dialog-restart-label]").textContent =
    `${replace ? "Restart" : "Start"} the ${units} using it`;

  dialog.showModal();
  form.elements.value.focus();
}
