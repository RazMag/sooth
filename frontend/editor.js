import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { syntaxHighlighting, HighlightStyle, indentUnit } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { quadletLanguage } from "./quadlet-lang.js";

// Colours come from CSS custom properties so the editor tracks light/dark.
const highlightStyle = HighlightStyle.define([
  { tag: t.comment, color: "var(--color-faint)", fontStyle: "italic" },
  { tag: t.heading, color: "var(--color-accent)", fontWeight: "600" },
  { tag: t.propertyName, color: "var(--color-ok)" },
  { tag: t.operator, color: "var(--color-muted)" },
]);

const theme = EditorView.theme({
  "&": { backgroundColor: "var(--color-surface)", color: "var(--color-ink)", fontSize: "0.85rem" },
  "&.cm-focused": { outline: "none" },
  ".cm-content": { fontFamily: "var(--font-mono, ui-monospace, monospace)", padding: "0.5rem 0" },
  ".cm-gutters": { backgroundColor: "var(--color-canvas)", color: "var(--color-faint)", border: "none" },
  ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--color-accent) 7%, transparent)" },
  ".cm-activeLineGutter": { backgroundColor: "color-mix(in srgb, var(--color-accent) 7%, transparent)" },
  ".cm-cursor": { borderLeftColor: "var(--color-ink)" },
  "&.cm-editor.cm-focused .cm-selectionBackground, ::selection": {
    backgroundColor: "color-mix(in srgb, var(--color-accent) 22%, transparent)",
  },
});

// The most recently mounted editor view (one editor per page). The host-var
// reference panel's insert buttons dispatch into this.
let currentView = null;

// Progressively enhances every `<textarea data-code-editor>` into a
// CodeMirror 6 editor and wires a debounced live-validate against POST
// /validate (the response HTML replaces #validate-status). The textarea stays
// in the DOM (hidden) and is kept in sync so the normal form submit still
// carries `contents`.
//
// The file name to validate against is either fixed (Edit page) or composed
// from a stem field plus either a static extension suffix (section "New"
// pages) or a `<select>` of kinds (`/units/new`).
export function initEditors() {
  wireInsertButtons();

  document.querySelectorAll("textarea[data-code-editor]").forEach((textarea) => {
    if (textarea.dataset.codeEditorInit) return;
    textarea.dataset.codeEditorInit = "1";

    const fixedName = textarea.dataset.fileName || "";
    const stemEl = textarea.dataset.stemInput ? document.querySelector(textarea.dataset.stemInput) : null;
    const suffix = textarea.dataset.suffix || "";
    const kindEl = textarea.dataset.kindSelect ? document.querySelector(textarea.dataset.kindSelect) : null;

    const currentFileName = () => {
      if (fixedName) return fixedName;
      const stem = stemEl ? stemEl.value.trim() : "";
      if (!stem) return "";
      return stem + (kindEl ? "." + kindEl.value : suffix);
    };

    let timer = null;
    const scheduleValidate = () => {
      clearTimeout(timer);
      timer = setTimeout(runValidate, 500);
    };
    const runValidate = () => {
      const statusEl = document.getElementById("validate-status");
      const fileName = currentFileName();
      if (!fileName || !statusEl) return;
      const body = new URLSearchParams({ file_name: fileName, contents: textarea.value });
      fetch("/validate", { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" }, body })
        .then((r) => r.text())
        .then((html) => {
          const el = document.getElementById("validate-status");
          if (el) el.outerHTML = html;
        })
        .catch(() => {
          /* transient hiccup -- the next keystroke retries */
        });
    };

    const sync = EditorView.updateListener.of((v) => {
      if (!v.docChanged) return;
      textarea.value = v.state.doc.toString();
      scheduleValidate();
    });

    const view = new EditorView({
      state: EditorState.create({
        doc: textarea.value,
        extensions: [
          lineNumbers(),
          highlightActiveLine(),
          highlightActiveLineGutter(),
          history(),
          drawSelection(),
          indentUnit.of("  "),
          EditorState.tabSize.of(2),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
          quadletLanguage,
          syntaxHighlighting(highlightStyle),
          EditorView.lineWrapping,
          theme,
          sync,
        ],
      }),
    });
    view.dom.classList.add("cm-host");
    view.dom.setAttribute("data-code-editor-host", "1");
    currentView = view;

    textarea.hidden = true;
    textarea.after(view.dom);

    if (stemEl) stemEl.addEventListener("input", scheduleValidate);
    if (kindEl) kindEl.addEventListener("change", scheduleValidate);
    scheduleValidate();
  });
}

// One delegated listener for the whole document; the buttons live in the
// host-var panel and insert a `${NAME}` reference at the editor's cursor.
function wireInsertButtons() {
  if (window.__soothInsertRefWired) return;
  window.__soothInsertRefWired = true;
  document.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-insert-ref]");
    if (!btn || !currentView) return;
    e.preventDefault();
    const ref = btn.getAttribute("data-insert-ref");
    const sel = currentView.state.selection.main;
    currentView.dispatch({
      changes: { from: sel.from, to: sel.to, insert: ref },
      selection: { anchor: sel.from + ref.length },
      scrollIntoView: true,
    });
    currentView.focus();
  });
}
