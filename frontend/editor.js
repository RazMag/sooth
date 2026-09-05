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

// Progressively enhances every `<textarea data-code-editor>` into a
// CodeMirror 6 editor and wires a debounced live-validate against POST
// /validate (same wire protocol the pre-CM5 version used: the response HTML
// replaces #validate-status). The textarea stays in the DOM (hidden) and is
// kept in sync so the normal form submit still carries `contents`.
export function initEditors() {
  document.querySelectorAll("textarea[data-code-editor]").forEach((textarea) => {
    if (textarea.dataset.codeEditorInit) return;
    textarea.dataset.codeEditorInit = "1";

    const fileInputSel = textarea.dataset.fileInput;
    const fileInput = fileInputSel ? document.querySelector(fileInputSel) : null;
    const fixedFileName = textarea.dataset.fileName || "";

    let timer = null;
    const scheduleValidate = () => {
      clearTimeout(timer);
      timer = setTimeout(runValidate, 500);
    };
    const runValidate = () => {
      // Looked up fresh: each response replaces this element's outerHTML.
      const statusEl = document.getElementById("validate-status");
      const fileName = fixedFileName || (fileInput ? fileInput.value.trim() : "");
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

    textarea.hidden = true;
    textarea.after(view.dom);

    if (fileInput) fileInput.addEventListener("input", scheduleValidate);
    scheduleValidate();
  });
}
