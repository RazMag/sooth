// Lets the Pod pages' "New containers"/"New networks"/"New volumes" fields
// add/remove independent editors purely client-side -- there's no server
// round trip to add a row. A new row is a clone of that field's hidden
// `<template>` (see `new_container_row`/`new_network_row`/`new_volume_row` in
// `src/web/templates/pods.rs`), with its `__ID__` placeholder replaced by a
// fresh id -- the row's markup lives in exactly one place per kind (that
// template), not duplicated here, so the live-add path and a 422
// redisplay's server-rendered rows can never drift apart.
//
// All three kinds share one implementation: each field's list/template/add
// button carry a matching `data-newres-list`/`data-newres-template`/
// `data-newres-add` value (`"newc"`/`"newnet"`/`"newvol"`, the same prefix
// the row's own field names use), so a row's id-counter and add/remove
// wiring never cross between fields. Only containers carry an env-var
// editor, but `initEditors`/`initEnvVars` are both idempotent per-element and
// scan the whole document, so calling both after every add is harmless for
// network/volume rows that have neither.

import { EditorView } from "@codemirror/view";
import { initEditors } from "./editor.js";
import { initEnvVars } from "./envvars.js";

export function initNewResourceRows() {
  document.querySelectorAll("[data-newres-add]").forEach((addBtn) => {
    if (addBtn.dataset.newresInit) return;
    addBtn.dataset.newresInit = "1";

    const prefix = addBtn.dataset.newresAdd;
    const list = document.querySelector(`[data-newres-list="${prefix}"]`);
    const template = document.querySelector(`[data-newres-template="${prefix}"]`);
    if (!list || !template) return;

    // Seed the id counter above whatever's already on the page -- rows from
    // a 422 redisplay reuse the ids the server assigned them, and a fresh
    // row must never collide with one of those.
    const existingIds = Array.from(
      list.querySelectorAll(`[data-newres-row] input[id^="${prefix}-"]`),
    )
      .map((el) => el.id.match(new RegExp(`^${prefix}-(\\d+)-name$`)))
      .filter(Boolean)
      .map((m) => Number(m[1]));
    let nextId = existingIds.length ? Math.max(...existingIds) + 1 : 1;

    const addRow = () => {
      const id = nextId++;
      const clone = template.content.cloneNode(true);
      clone.querySelectorAll("[id], [name], [for], [data-stem-input]").forEach((el) => {
        if (el.id) el.id = el.id.replace("__ID__", id);
        if (el.name) el.name = el.name.replace("__ID__", id);
        if (el.htmlFor) el.htmlFor = el.htmlFor.replace("__ID__", id);
        if (el.dataset.stemInput) {
          el.dataset.stemInput = el.dataset.stemInput.replace("__ID__", id);
        }
      });
      list.append(clone);
      initEditors();
      initEnvVars();
    };

    addBtn.addEventListener("click", addRow);

    list.addEventListener("click", (e) => {
      const btn = e.target.closest("[data-newres-remove]");
      if (!btn) return;
      const row = btn.closest("[data-newres-row]");
      if (!row) return;
      const host = row.querySelector("[data-code-editor-host]");
      if (host) {
        const view = EditorView.findFromDOM(host);
        if (view) view.destroy();
      }
      row.remove();
    });
  });
}
