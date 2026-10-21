import { useEffect, useLayoutEffect, useRef } from "react";
import { EditorState, Prec, type Extension } from "@codemirror/state";
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
import { oneDarkHighlightStyle } from "@codemirror/theme-one-dark";
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

// Reuse the YAML token groups without coupling their contrast to a dark palette.
const editorHighlightStyle = HighlightStyle.define(
  oneDarkHighlightStyle.specs.map((spec) => ({
    ...spec,
    color: spec.fontStyle === "italic"
      ? "hsl(var(--muted-foreground))"
      : "hsl(var(--foreground))",
    fontWeight: spec.fontStyle === "italic" ? undefined : "500",
  })),
);

// Application tokens win over CodeMirror's base styles in both color modes.
const editorTheme = EditorView.theme({
  "&": {
    backgroundColor: "hsl(var(--background))",
    fontSize: "14px",
    color: "hsl(var(--foreground))",
  },
  ".cm-scroller": {
    fontFamily:
      "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, 'Liberation Mono', monospace",
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
  readOnly: boolean,
  minHeight: string,
  onChange: (value: string) => void,
): Extension[] {
  const extensions: Extension[] = [
    lineNumbers(),
    history(),
    indentOnInput(),
    bracketMatching(),
    yaml(),
    syntaxHighlighting(editorHighlightStyle),
    Prec.high(editorTheme),
    EditorView.theme({ "&": { minHeight } }),
    EditorView.lineWrapping,
    EditorState.readOnly.of(readOnly),
    EditorView.editable.of(!readOnly),
  ];
  if (readOnly) {
    return extensions;
  }
  extensions.push(
    highlightActiveLine(),
    highlightActiveLineGutter(),
    keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
    EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        onChange(update.state.doc.toString());
      }
    }),
  );
  return extensions;
}

function CodeEditorInner({
  value,
  onChange,
  readOnly = false,
  minHeight = "14rem",
  "aria-label": ariaLabel,
}: CodeEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  // Keep the latest onChange without recreating the editor on every render.
  const onChangeRef = useRef(onChange);
  // Read `value` through a ref inside the creation effect below, so the effect
  // depends only on the structural props it must rebuild for.
  const valueRef = useRef(value);
  // useLayoutEffect (not render body) to avoid the react-hooks/refs lint rule.
  useLayoutEffect(() => {
    onChangeRef.current = onChange;
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
        extensions: buildExtensions(readOnly, minHeight, (next) =>
          onChangeRef.current(next),
        ),
      }),
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, [readOnly, minHeight]);

  // Reconcile external value changes (programmatic) without clobbering the
  // cursor on echoes of the user's own edits.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (current !== value) {
      view.dispatch({
        changes: { from: 0, to: current.length, insert: value },
      });
    }
  }, [value]);

  return (
    <div
      ref={hostRef}
      aria-label={ariaLabel}
      className="overflow-hidden rounded-md border border-border bg-background"
    />
  );
}

export default CodeEditorInner;
