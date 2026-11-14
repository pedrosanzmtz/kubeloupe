// CodeMirror setup shared by the editor and the diff view. Imported only
// through dynamic `import()` (see CodeEditor.svelte / DiffView.svelte), so it
// is a separate chunk that loads when an editor first opens.

import { basicSetup } from "codemirror";
import { indentWithTab } from "@codemirror/commands";
import { yaml } from "@codemirror/lang-yaml";
import { HighlightStyle, indentUnit, syntaxHighlighting } from "@codemirror/language";
import { Compartment, EditorState, StateEffect, StateField } from "@codemirror/state";
import type { Extension } from "@codemirror/state";
import { Decoration, EditorView, keymap } from "@codemirror/view";
import type { DecorationSet } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

const MONO = 'ui-monospace, "SF Mono", Menlo, Consolas, monospace';

/** Dark theme from the variables in app.css. */
const theme = EditorView.theme(
  {
    "&": {
      height: "100%",
      backgroundColor: "#0d1013",
      color: "#d6dae0",
      fontSize: "12px",
    },
    ".cm-scroller": { fontFamily: MONO, lineHeight: "1.5" },
    ".cm-content": { caretColor: "var(--text)" },
    "&.cm-focused": { outline: "none" },
    "&.cm-focused .cm-cursor": { borderLeftColor: "var(--text)" },
    ".cm-gutters": {
      backgroundColor: "#0d1013",
      color: "var(--text-muted)",
      border: "none",
      borderRight: "1px solid var(--border)",
    },
    ".cm-activeLine": { backgroundColor: "rgba(255, 255, 255, 0.03)" },
    ".cm-activeLineGutter": { backgroundColor: "rgba(255, 255, 255, 0.05)", color: "var(--text)" },
    "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, ::selection":
      { backgroundColor: "rgba(79, 156, 247, 0.3)" },
    ".cm-selectionMatch": { backgroundColor: "rgba(79, 156, 247, 0.15)" },
    "&.cm-focused .cm-matchingBracket": { backgroundColor: "rgba(63, 178, 127, 0.25)", outline: "none" },
    ".cm-searchMatch": { backgroundColor: "rgba(245, 165, 36, 0.25)", outline: "1px solid var(--amber)" },
    ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "rgba(245, 165, 36, 0.5)" },
    ".cm-foldPlaceholder": {
      backgroundColor: "var(--bg-hover)",
      border: "1px solid var(--border)",
      color: "var(--text-muted)",
    },
    ".cm-tooltip, .cm-panels": {
      backgroundColor: "var(--bg-raised)",
      color: "var(--text)",
      border: "1px solid var(--border)",
    },
    ".cm-panels input, .cm-panels button": { fontSize: "12px" },
    ".cm-textfield": {
      backgroundColor: "var(--bg)",
      color: "var(--text)",
      border: "1px solid var(--border)",
      borderRadius: "4px",
    },
    ".cm-button": {
      backgroundImage: "none",
      backgroundColor: "var(--bg-hover)",
      color: "var(--text)",
      border: "1px solid var(--border)",
      borderRadius: "4px",
    },
    ".cm-errorLine": { backgroundColor: "rgba(229, 72, 77, 0.18)" },
    // Diff view (@codemirror/merge).
    ".cm-changedLine, .cm-merge-b .cm-changedLine": { backgroundColor: "rgba(63, 178, 127, 0.12)" },
    ".cm-changedText, .cm-merge-b .cm-changedText": { background: "rgba(63, 178, 127, 0.35)" },
    ".cm-deletedChunk, .cm-merge-a .cm-changedLine": { backgroundColor: "rgba(229, 72, 77, 0.12)" },
    ".cm-deletedChunk .cm-deletedText, .cm-merge-a .cm-changedText": { background: "rgba(229, 72, 77, 0.35)" },
    ".cm-changeGutter": { width: "3px" },
  },
  { dark: true },
);

const highlight = HighlightStyle.define([
  { tag: [t.propertyName, t.definition(t.propertyName)], color: "#7cb7ff" },
  { tag: [t.string, t.special(t.string)], color: "#9fd6a8" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "#f5c46a" },
  { tag: [t.comment, t.lineComment], color: "#6b7480", fontStyle: "italic" },
  { tag: [t.keyword, t.typeName, t.labelName], color: "#d7a6ff" },
  { tag: [t.punctuation, t.separator, t.squareBracket, t.brace], color: "#9aa3b0" },
  { tag: t.meta, color: "#9aa3b0" },
  { tag: t.invalid, color: "#f5a3a5" },
]);

/** YAML with the app's look: what both the editor and the diff use. */
export function yamlLook(): Extension {
  return [yaml(), theme, syntaxHighlighting(highlight)];
}

function label(text: string): Extension {
  return EditorView.contentAttributes.of({ "aria-label": text });
}

// One line marked as the location of an error.
const markLine = StateEffect.define<number | null>();
const errorLine = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    deco = deco.map(tr.changes);
    for (const e of tr.effects) {
      if (!e.is(markLine)) continue;
      const n = e.value;
      if (n === null || n < 1 || n > tr.state.doc.lines) {
        deco = Decoration.none;
      } else {
        const line = tr.state.doc.line(n);
        deco = Decoration.set([Decoration.line({ class: "cm-errorLine" }).range(line.from)]);
      }
    }
    return deco;
  },
  provide: (f) => EditorView.decorations.from(f),
});

export interface EditorOptions {
  value: string;
  readonly: boolean;
  label: string;
  onChange: (value: string) => void;
}

/** What `CodeEditor.svelte` drives. */
export interface EditorHandle {
  getValue(): string;
  setValue(value: string): void;
  setReadonly(readonly: boolean): void;
  /** Mark a 1-based line (and scroll to it), or clear the mark. */
  markLine(line: number | null): void;
  destroy(): void;
}

export function createEditor(parent: HTMLElement, opts: EditorOptions): EditorHandle {
  const ro = new Compartment();
  const readonly = (on: boolean) => [
    EditorState.readOnly.of(on),
    EditorView.editable.of(!on),
    EditorView.contentAttributes.of({ "aria-readonly": String(on) }),
  ];
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: opts.value,
      extensions: [
        basicSetup,
        keymap.of([indentWithTab]),
        indentUnit.of("  "),
        EditorState.tabSize.of(2),
        yamlLook(),
        label(opts.label),
        errorLine,
        ro.of(readonly(opts.readonly)),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) opts.onChange(u.state.doc.toString());
        }),
      ],
    }),
  });
  return {
    getValue: () => view.state.doc.toString(),
    setValue(value) {
      if (value === view.state.doc.toString()) return;
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
    },
    setReadonly(on) {
      view.dispatch({ effects: ro.reconfigure(readonly(on)) });
    },
    markLine(line) {
      const effects: StateEffect<unknown>[] = [markLine.of(line)];
      if (line !== null && line >= 1 && line <= view.state.doc.lines) {
        effects.push(EditorView.scrollIntoView(view.state.doc.line(line).from, { y: "center" }));
      }
      view.dispatch({ effects });
    },
    destroy: () => view.destroy(),
  };
}

export type DiffLayout = "unified" | "split";

/** A read-only diff of `original` (left / removed) and `modified`. */
export async function createDiff(
  parent: HTMLElement,
  original: string,
  modified: string,
  layout: DiffLayout,
  name: string,
): Promise<{ destroy(): void }> {
  const { MergeView, unifiedMergeView } = await import("@codemirror/merge");
  const common = [
    EditorView.lineWrapping,
    EditorState.readOnly.of(true),
    EditorView.editable.of(false),
    yamlLook(),
  ];
  // Long objects: show the changes with a few lines of context.
  const collapseUnchanged = { margin: 3, minSize: 6 };
  if (layout === "unified") {
    const view = new EditorView({
      parent,
      doc: modified,
      extensions: [
        ...common,
        label(name),
        unifiedMergeView({ original, mergeControls: false, highlightChanges: true, gutter: true, collapseUnchanged }),
      ],
    });
    return view;
  }
  const merge = new MergeView({
    parent,
    a: { doc: original, extensions: [...common, label(`${name}: server`)] },
    b: { doc: modified, extensions: [...common, label(`${name}: yours`)] },
    highlightChanges: true,
    gutter: true,
    collapseUnchanged,
  });
  return merge;
}
