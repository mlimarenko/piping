import { lazy, Suspense } from "react";
import { Loader2 } from "lucide-react";

export interface CodeEditorProps {
  /** The current document text. The component is fully controlled. */
  value: string;
  /** Receives the new document text on every edit. */
  onChange: (value: string) => void;
  /** Stable identity for independent undo history and selection. Defaults to one YAML document. */
  documentId?: string;
  /** YAML highlighting and diagnostics are enabled for .yaml and .yml files. */
  fileName?: string;
  /** Called on Mod-s when provided. */
  onSave?: () => void;
  /** Render the editor non-editable (history / diff view). */
  readOnly?: boolean;
  /** CSS min-height for the editor body (default `14rem`). */
  minHeight?: string;
  /** Definite canvas height; omit for natural-height form editors. */
  height?: string;
  /** Accessible label for the editor region. */
  "aria-label"?: string;
}

// The heavy CodeMirror implementation is code-split into its own async chunk:
// React.lazy emits a separate bundle so `@codemirror/*` never enters the main
// entry. Only a `import type` link exists between this wrapper and the inner
// module, which is erased at build time.
const CodeEditorInner = lazy(() => import("@/features/playbooks/CodeEditorInner"));

const EDITOR_FALLBACK_CLASS =
  "flex items-center justify-center rounded-md border border-border bg-background text-sm text-muted-foreground";

/**
 * Lazy-loaded CodeMirror 6 editor primitive for Ansible playbooks. YAML syntax
 * highlighting, line numbers, monospace, opaque editor theme. Kept inside the
 * playbooks feature (not `shared/`) to avoid co-development contention; it can
 * be promoted to a shared primitive later.
 */
function CodeEditor({ minHeight = "14rem", height, ...props }: CodeEditorProps) {
  return (
    <Suspense
      fallback={
        <div className={EDITOR_FALLBACK_CLASS} style={{ minHeight, height }}>
          <Loader2 className="h-4 w-4 animate-spin" />
        </div>
      }
    >
      <CodeEditorInner minHeight={minHeight} height={height} {...props} />
    </Suspense>
  );
}

export default CodeEditor;
