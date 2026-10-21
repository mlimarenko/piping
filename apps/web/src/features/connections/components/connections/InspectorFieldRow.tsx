import { useState, type ElementType, type MouseEvent } from "react";
import { Link } from "react-router";
import { Check, Copy } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import { writeBrowserClipboardText } from "@/shared/lib/clipboard";
import InspectorFieldLabel from "@/shared/ui-shared/InspectorFieldLabel";
import { hostValue } from "./hostPresentation";

/** Simplified `TFunction` shape threaded through the inspector's read-only rows. */
type Translate = (k: string, opts?: Record<string, unknown>) => string;

function inspectorCopyValue(value?: string | null): string | null {
  if (value == null) return null;
  return value.trim() ? value : null;
}

function notifyClipboardWriteUnavailable(t: Translate): void {
  notify("warning", t("connections.clipboard.receiveUnavailable"), {
    description: t("connections.clipboard.receiveFailedDescription"),
    source: "connections",
  });
}

/**
 * Copy affordance for a read-only inspector value. Renders nothing when the
 * value is empty; shows an in-place checkmark on success and a warning toast
 * when the clipboard is unavailable. Shared by `ConnectionInspector` and
 * `TreeNodeInspector`, which each kept a private copy of this button.
 */
export const InspectorCopyButton = ({ label, value, t, className }: {
  label: string;
  value?: string | null;
  t: Translate;
  className?: string;
}) => {
  const [copied, setCopied] = useState(false);
  const copyValue = inspectorCopyValue(value);

  if (!copyValue) return null;

  const handleCopy = (event: MouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
    void writeBrowserClipboardText(copyValue).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      },
      () => notifyClipboardWriteUnavailable(t),
    );
  };

  const ButtonIcon = copied ? Check : Copy;
  const title = copied
    ? t("connections.inspector.copiedFieldValue", { field: label })
    : t("connections.inspector.copyFieldValue", { field: label });

  return (
    <button
      type="button"
      className={cn(
        "inline-flex size-9 shrink-0 items-center justify-center rounded-sm text-muted-foreground transition-colors",
        "hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
        copied && "text-primary",
        className,
      )}
      aria-label={title}
      title={title}
      onClick={handleCopy}
    >
      <ButtonIcon className="h-3 w-3" aria-hidden="true" />
    </button>
  );
};

export interface InspectorFieldRowProps {
  icon?: ElementType;
  label: string;
  help?: string;
  value?: string | null;
  mono?: boolean;
  /** Renders the value as an inventory edit deep-link (read-only operators). */
  href?: string | null;
  /** When set, the value is a button opening the in-workspace edit form. */
  onActivate?: () => void;
  t: Translate;
}

/**
 * Read-only field row: `icon + label + help` on the left (via the shared
 * {@link InspectorFieldLabel}) and the value (link/button/text) plus a copy
 * affordance on the right. Replaces the parallel `InfoRow` (ConnectionInspector)
 * and `LinkedRow` (TreeNodeInspector) renderers.
 */
export const InspectorFieldRow = ({ icon: Icon, label, help, value, mono, href, onActivate, t }: InspectorFieldRowProps) => (
  <div className="group/field grid grid-cols-[minmax(0,42%)_minmax(0,1fr)] items-start gap-2 text-sm py-1">
    <div className="min-w-0 text-muted-foreground">
      <InspectorFieldLabel icon={Icon} label={label} help={help} iconClassName="h-2.5 w-2.5" />
    </div>
    <div className="flex min-w-0 items-start justify-end gap-1.5">
      {onActivate ? (
        <button
          type="button"
          onClick={onActivate}
          title={t("connections.inspector.openEdit")}
          className={cn("min-w-0 min-h-6 whitespace-pre-wrap [overflow-wrap:anywhere] rounded-sm text-right text-primary hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring", mono && "font-mono text-xs")}
        >
          {hostValue(value)}
        </button>
      ) : href ? (
        <Link
          to={href}
          title={t("connections.inspector.openInInventory")}
          className={cn("min-w-0 whitespace-pre-wrap [overflow-wrap:anywhere] text-right text-primary hover:underline", mono && "font-mono text-xs")}
        >
          {hostValue(value)}
        </Link>
      ) : (
        <span className={cn("min-w-0 whitespace-pre-wrap [overflow-wrap:anywhere] text-right text-foreground", mono && "font-mono text-xs")}>{hostValue(value)}</span>
      )}
      <InspectorCopyButton label={label} value={value} t={t} />
    </div>
  </div>
);
