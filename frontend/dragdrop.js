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
  for (const el of document.querySelectorAll(".drag-over")) {
    el.classList.remove("drag-over");
  }
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
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    clearDropHints();
    e.target.closest("tr").classList.add("drag-over");
  });

  document.addEventListener("dragleave", (e) => {
    e.target.closest?.("tr")?.classList.remove("drag-over");
  });

  document.addEventListener("drop", (e) => {
    if (!drag) return;
    const target = dropGroup(e.target);
    if (target == null || invalidGroupTarget(target)) return;
    e.preventDefault();
    const d = drag;
    drag = null;
    clearDropHints();
    if (d.kind === "unit") {
      post(d.url, { group: target });
    } else {
      post("/groups/move", { group: d.path, parent: target });
    }
  });
}
