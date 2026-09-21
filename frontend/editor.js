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

// Where a host-var `${NAME}` reference goes when a chip is clicked: either a
// CodeMirror instance ({kind:"codemirror", view}) or a plain insertable
// input ({kind:"input", el}, e.g. an env-var value field). A page can now
// carry more than one of each (one raw editor / one env editor per inline
// "new container" row on the Pod pages), so this tracks whichever was most
// recently *focused* rather than "whichever mounted last" -- the old
// single `currentView` only ever worked by accident when a page happened to
// have exactly one editor and nothing else insertable on it.
let insertTarget = null;

// Progressively enhances every `<textarea data-code-editor>` into a
// CodeMirror 6 editor and wires a debounced live-validate against POST
// /validate. The textarea stays in the DOM (hidden) and is kept in sync so
// the normal form submit still carries its field. Each instance's live
// validate result renders into the `.validate-status` div immediately after
// its own textarea in the markup (captured once at mount time), not a
// document-wide id -- `code_editor()` no longer emits one, since a page can
// carry more than one editor.
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
    const statusHolder = textarea.nextElementSibling;

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
      const fileName = currentFileName();
      if (!fileName || !statusHolder) return;
      const body = new URLSearchParams({ file_name: fileName, contents: textarea.value });
      fetch("/validate", { method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" }, body })
        .then((r) => r.text())
        .then((html) => {
          statusHolder.innerHTML = html;
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
          EditorView.domEventHandlers({
            focus: () => {
              insertTarget = { kind: "codemirror", view };
            },
          }),
        ],
      }),
    });
    view.dom.classList.add("cm-host");
    view.dom.setAttribute("data-code-editor-host", "1");
    if (!insertTarget) insertTarget = { kind: "codemirror", view };

    textarea.hidden = true;
    textarea.after(view.dom);

    if (stemEl) stemEl.addEventListener("input", scheduleValidate);
    if (kindEl) kindEl.addEventListener("change", scheduleValidate);
    scheduleValidate();
  });
}

// One delegated listener for the whole document; the host-var panel's
// buttons insert a `${NAME}` reference into whichever editor/input was last
// focused. Plain insertable fields (env-var values) opt in with
// `data-insertable`; CodeMirror instances register themselves via the
// `focus` domEventHandler wired in `initEditors()` above.
function wireInsertButtons() {
  if (window.__soothInsertRefWired) return;
  window.__soothInsertRefWired = true;

  document.addEventListener(
    "focusin",
    (e) => {
      if (e.target.matches && e.target.matches("[data-insertable]")) {
        insertTarget = { kind: "input", el: e.target };
      }
    },
    true,
  );

  document.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-insert-ref]");
    if (!btn || !insertTarget) return;
    e.preventDefault();
    const ref = btn.getAttribute("data-insert-ref");

    if (insertTarget.kind === "codemirror") {
      const view = insertTarget.view;
      const sel = view.state.selection.main;
      view.dispatch({
        changes: { from: sel.from, to: sel.to, insert: ref },
        selection: { anchor: sel.from + ref.length },
        scrollIntoView: true,
      });
      view.focus();
    } else {
      const el = insertTarget.el;
      const start = el.selectionStart ?? el.value.length;
      const end = el.selectionEnd ?? el.value.length;
      el.setRangeText(ref, start, end, "end");
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.focus();
    }
  });
}
