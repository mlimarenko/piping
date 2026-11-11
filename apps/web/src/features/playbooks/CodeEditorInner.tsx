import { useEffect, useLayoutEffect, useRef } from "react";
import { Annotation, Compartment, EditorState, Prec, Transaction, type Extension } from "@codemirror/state";
import {
  EditorView,
  lineNumbers,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, HighlightStyle, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { yaml } from "@codemirror/lang-yaml";
import { linter, type Diagnostic } from "@codemirror/lint";
import { search, searchKeymap } from "@codemirror/search";
import { tags } from "@lezer/highlight";
import { parseAllDocuments } from "yaml";
import type { CodeEditorProps } from "@/features/playbooks/CodeEditor";

/**
 * The heavy CodeMirror 6 implementation. Imported only through `React.lazy`
 * from {@link CodeEditor}, so every `@codemirror/*` module lands in its own
 * async chunk and never bloats the main entry bundle.
 *
 * Controlled component: the parent owns `value`; local edits are pushed up via
 * `onChange`. External `value` changes (e.g. switching to a historical version)
 * are reconciled into the view without resetting the cursor on no-op echoes.
 */

const editorHighlightStyle = HighlightStyle.define([
  { tag: tags.propertyName, color: "hsl(var(--primary))", fontWeight: "600" },
  { tag: tags.string, color: "hsl(var(--destructive))" },
  { tag: [tags.number, tags.bool, tags.null], color: "hsl(var(--primary))", fontWeight: "600" },
  { tag: tags.comment, color: "hsl(var(--muted-foreground))", fontStyle: "italic" },
  { tag: tags.meta, color: "hsl(var(--muted-foreground))" },
]);

const externalChange = Annotation.define<boolean>();
const yamlDiagnostics = linter((view) => {
  const text = view.state.doc.toString();
  return parseAllDocuments(text, { uniqueKeys: false }).flatMap((doc) =>
    doc.errors.map((error): Diagnostic => ({
      from: Math.min(error.pos[0], text.length),
      to: Math.min(Math.max(error.pos[1], error.pos[0] + 1), text.length),
      severity: "error",
      message: error.message,
    })),
  );
});

// Application tokens win over CodeMirror's base styles in both color modes.
const editorTheme = EditorView.theme({
  "&": {
    backgroundColor: "hsl(var(--background))",
    fontSize: "14px",
    color: "hsl(var(--foreground))",
  },
  ".cm-scroller": {
    fontFamily: "var(--font-family-mono)",
    fontVariantLigatures: "none",
    lineHeight: "1.55",
  },
  ".cm-gutters": {
    backgroundColor: "hsl(var(--muted))",
    border: "none",
    color: "hsl(var(--muted-foreground))",
  },
  ".cm-content": {
    caretColor: "hsl(var(--foreground))",
  },
  "&.cm-focused": {
    outline: "none",
  },
  ".cm-activeLine, .cm-activeLineGutter": {
    backgroundColor: "hsl(var(--muted))",
  },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "hsl(var(--accent))",
  },
});

function buildExtensions(
  onChange: (value: string) => void,
  onSave: () => boolean,
  language: Compartment,
  editable: Compartment,
  height: Compartment,
): Extension[] {
  return [
    lineNumbers(),
    history(),
    indentOnInput(),
    bracketMatching(),
    language.of([]),
    syntaxHighlighting(editorHighlightStyle),
    Prec.high(editorTheme),
    height.of([]),
    EditorView.lineWrapping,
    editable.of([]),
    search(),
    highlightActiveLine(),
    highlightActiveLineGutter(),
    keymap.of([{ key: "Mod-s", run: onSave }, ...searchKeymap, ...defaultKeymap, ...historyKeymap, indentWithTab]),
    EditorView.updateListener.of((update) => {
      if (update.docChanged && !update.transactions.some((tr) => tr.annotation(externalChange))) {
        onChange(update.state.doc.toString());
      }
    }),
  ];
}

function CodeEditorInner({
  value,
  onChange,
  readOnly = false,
  minHeight = "14rem",
  height,
  documentId = "default",
  fileName,
  onSave,
  "aria-label": ariaLabel,
}: CodeEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  // Keep the latest onChange without recreating the editor on every render.
  const onChangeRef = useRef(onChange);
  const onSaveRef = useRef(onSave);
  const statesRef = useRef(new Map<string, EditorState>());
  const activeIdRef = useRef(documentId);
  const languageRef = useRef(new Compartment());
  const editableRef = useRef(new Compartment());
  const heightRef = useRef(new Compartment());
  // Read `value` through a ref inside the creation effect below, so the effect
  // depends only on the structural props it must rebuild for.
  const valueRef = useRef(value);
  // useLayoutEffect (not render body) to avoid the react-hooks/refs lint rule.
  useLayoutEffect(() => {
    onChangeRef.current = onChange;
    onSaveRef.current = onSave;
    valueRef.current = value;
  });

  // Recreate the editor only when structural props change.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: valueRef.current,
        extensions: buildExtensions((next) => onChangeRef.current(next), () => {
          if (!onSaveRef.current) return false;
          onSaveRef.current();
          return true;
        }, languageRef.current, editableRef.current, heightRef.current),
      }),
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, []);

  // Reconcile external value changes (programmatic) without clobbering the
  // cursor on echoes of the user's own edits.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    if (activeIdRef.current !== documentId) {
      statesRef.current.set(activeIdRef.current, view.state);
      const saved = statesRef.current.get(documentId);
      view.setState(saved ?? EditorState.create({
        doc: value,
        extensions: buildExtensions((next) => onChangeRef.current(next), () => {
          if (!onSaveRef.current) return false;
          onSaveRef.current();
          return true;
        }, languageRef.current, editableRef.current, heightRef.current),
      }));
      activeIdRef.current = documentId;
    }
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({
        changes: { from: 0, to: current.length, insert: value },
        annotations: [externalChange.of(true), Transaction.addToHistory.of(false)],
      });
    }
    const isYaml = fileName === undefined || /\.ya?ml$/i.test(fileName);
    view.dispatch({ effects: [
      languageRef.current.reconfigure(isYaml ? [yaml(), yamlDiagnostics] : []),
      editableRef.current.reconfigure([EditorState.readOnly.of(readOnly), EditorView.editable.of(!readOnly)]),
      heightRef.current.reconfigure(EditorView.theme({ "&": { minHeight, height: height ?? "auto" } })),
    ] });
  }, [value, documentId, fileName, readOnly, minHeight, height]);

  return (
    <div
      ref={hostRef}
      aria-label={ariaLabel}
      className="overflow-hidden rounded-md border border-border bg-background"
      style={{ height }}
    />
  );
}

export default CodeEditorInner;
