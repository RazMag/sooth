// Drag rows and group headers around the list tables to re-file them.
//
//  * a unit row, grabbed by its grip -> POST {unit}/move  (group = drop target)
//  * a group header, grabbed by its grip -> POST /groups/move (parent = target)
//
// Every request carries `HX-Request` so the server answers 204; the table then
// redraws from the `units-changed` SSE broadcast that the move handlers send.

function csrfToken() {
  return document.querySelector('input[name="csrf_token"]')?.value || "";
}

// The group a drop on `el` targets: a group header -> its path; any member
// row -> that member's group; a bare root row -> "" (the quadlet-dir root).
// null when `el` isn't part of a droppable row.
function dropGroup(el) {
  const tr = el.closest?.("tr");
  if (!tr) return null;
  if (tr.classList.contains("group-row")) return tr.dataset.group ?? null;
  if (tr.dataset.groupMember != null) return tr.dataset.groupMember;
  if (tr.dataset.moveUrl) return "";
  return null;
}

function clearDropHints() {
  for (const el of document.querySelectorAll(".drag-over, .drag-forbidden")) {
    el.classList.remove("drag-over", "drag-forbidden");
  }
}

// The git-synced group paths for `el`'s table (see `data-synced-groups` on
// `table.data-table` in templates/list.rs) -- read fresh each time rather
// than cached, since the attribute can go stale mid-page (a sync added or
// removed elsewhere) and a fresh read costs nothing drag-loop-wise.
function syncedGroups(el) {
  const raw = el.closest?.("table.data-table")?.dataset.syncedGroups || "";
  return raw ? raw.split(",") : [];
}

// Whether `group` is (or sits inside) a git-synced directory for `el`'s
// table -- such a target is managed by `quadlet::gitsync`, and the next
// sync would just discard anything dropped there by hand. The server-side
// checks in `web::core` (`synced_destination` / `synced_source`) are the
// real gate; this is what lets the drop be refused with an explanation
// instead of a silent, confusing failure.
function isSyncedTarget(el, group) {
  return syncedGroups(el).some((g) => group === g || group.startsWith(g + "/"));
}

// A small floating banner shown for as long as a drag hovers a synced
// (undroppable) target -- the red row highlight alone doesn't say *why* a
// drop is refused, this spells it out. Created once and reused/hidden
// rather than added and removed, so a fast drag across several rows
// doesn't churn the DOM.
function showForbiddenHint(target) {
  let hint = document.getElementById("drag-forbidden-hint");
  if (!hint) {
    hint = document.createElement("div");
    hint.id = "drag-forbidden-hint";
    hint.className = "banner banner-error drag-forbidden-hint";
    hint.hidden = true;
    document.body.appendChild(hint);
  }
  hint.textContent = `"${target}" is synced from git — can't drop here`;
  hint.hidden = false;
}

function hideForbiddenHint() {
  const hint = document.getElementById("drag-forbidden-hint");
  if (hint) hint.hidden = true;
}

function post(url, params) {
  fetch(url, {
    method: "POST",
    body: new URLSearchParams({ csrf_token: csrfToken(), ...params }),
    headers: { "HX-Request": "true" },
    credentials: "same-origin",
  }).catch(() => {});
}

export function initDragDrop() {
  if (initDragDrop._wired) return;
  initDragDrop._wired = true;

  // { kind: "unit", url } | { kind: "group", path } | null
  let drag = null;

  document.addEventListener("dragstart", (e) => {
    const handle = e.target.closest?.(".drag-handle");
    if (!handle) return;
    const tr = handle.closest("tr");
    if (handle.classList.contains("group-drag")) {
      drag = tr?.dataset.group ? { kind: "group", path: tr.dataset.group } : null;
    } else {
      drag = tr?.dataset.moveUrl ? { kind: "unit", url: tr.dataset.moveUrl } : null;
    }
    if (!drag) return;
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", drag.kind);
    tr.classList.add("dragging");
  });

  document.addEventListener("dragend", () => {
    drag = null;
    for (const el of document.querySelectorAll("tr.dragging")) {
      el.classList.remove("dragging");
    }
    clearDropHints();
    hideForbiddenHint();
  });

  // A group can't be dropped onto itself or one of its own descendants.
  function invalidGroupTarget(target) {
    return (
      drag.kind === "group" &&
      (target === drag.path || target.startsWith(drag.path + "/"))
    );
  }

  document.addEventListener("dragover", (e) => {
    if (!drag) return;
    const target = dropGroup(e.target);
    if (target == null || invalidGroupTarget(target)) return;
    // Still preventDefault (so `drop` fires below and can explain why) --
    // only the cursor and highlight say "not here", the drop itself is
    // rejected with a message rather than swallowed silently.
    e.preventDefault();
    const forbidden = isSyncedTarget(e.target, target);
    e.dataTransfer.dropEffect = forbidden ? "none" : "move";
    clearDropHints();
    e.target.closest("tr").classList.add(forbidden ? "drag-forbidden" : "drag-over");
    if (forbidden) {
      showForbiddenHint(target);
    } else {
      hideForbiddenHint();
    }
  });

  document.addEventListener("dragleave", (e) => {
    e.target.closest?.("tr")?.classList.remove("drag-over", "drag-forbidden");
    hideForbiddenHint();
  });

  document.addEventListener("drop", (e) => {
    if (!drag) return;
    const target = dropGroup(e.target);
    if (target == null || invalidGroupTarget(target)) return;
    e.preventDefault();
    const d = drag;
    drag = null;
    clearDropHints();
    hideForbiddenHint();
    if (isSyncedTarget(e.target, target)) {
      alert(
        `"${target}" is synced from a git repository and is managed by the remote ` +
          "repo (see the Git Sync page) — it can't be used as a move target."
      );
      return;
    }
    if (d.kind === "unit") {
      post(d.url, { group: target });
    } else {
      post("/groups/move", { group: d.path, parent: target });
    }
  });
}
