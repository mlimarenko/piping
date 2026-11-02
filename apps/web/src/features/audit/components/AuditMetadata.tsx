import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type { AuditEntry } from "@/shared/api/types";

// Bound both traversal and mounted structured values, including deeply nested input.
const STRUCTURED_VALUE_LIMIT = 100;

function renderStructuredMetadata(details: NonNullable<AuditEntry["details"]>, t: TFunction) {
  let remaining = STRUCTURED_VALUE_LIMIT;
  let omitted = false;

  function renderValue(value: unknown, depth: number): ReactNode {
    remaining -= 1;
    if (value !== null && typeof value === "object") {
      const array = Array.isArray(value);
      const entries = Object.entries(value);
      const children: ReactNode[] = [];
      for (const [key, child] of entries) {
        if (remaining === 0) {
          omitted = true;
          break;
        }
        children.push(array ? (
          <li key={key} className="min-w-0 pl-1">{renderValue(child, depth + 1)}</li>
        ) : (
          <div key={key} className="grid min-w-0 gap-1">
            <dt className="whitespace-pre-wrap font-mono text-xs [overflow-wrap:anywhere]">
              {key === "" ? t("auditLog.metadataEmptyKey") : key}
            </dt>
            <dd className="min-w-0">{renderValue(child, depth + 1)}</dd>
          </div>
        ));
      }
      const label = t(array ? "auditLog.metadataArray" : "auditLog.metadataObject", { count: entries.length });
      if (entries.length === 0) return <span className="text-muted-foreground">{label}</span>;
      return (
        <details open className="min-w-0">
          <summary className="cursor-pointer rounded py-1 text-xs text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{label}</summary>
          <div className={depth < 3 ? "min-w-0 border-l border-border pl-3" : "min-w-0"}>
            {array ? (
              <ol className="grid min-w-0 list-decimal gap-3 pl-5">{children}</ol>
            ) : (
              <dl className="grid min-w-0 gap-3">{children}</dl>
            )}
          </div>
        </details>
      );
    }
    const type = value === null ? "null" : typeof value;
    return (
      <div className="grid min-w-0 gap-1">
        <span className="text-xs text-muted-foreground">{t(`auditLog.metadataTypes.${type}`)}</span>
        <span className="whitespace-pre-wrap text-sm [overflow-wrap:anywhere]">
          {value === "" ? t("auditLog.metadataEmptyString") : String(value)}
        </span>
      </div>
    );
  }

  const structured = renderValue(details, 0);
  return { structured, omitted };
}

export function AuditMetadata({ details }: { details: NonNullable<AuditEntry["details"]> }) {
  const { t } = useTranslation();
  const [rawOpen, setRawOpen] = useState(false);
  const { structured, omitted } = renderStructuredMetadata(details, t);
  return (
    <section aria-label={t("auditLog.metadata")} className="grid min-w-0 gap-3 rounded-md border border-border p-3">
      <h3 className="text-sm font-medium">{t("auditLog.metadata")}</h3>
      {structured}
      {omitted && <p className="text-xs text-muted-foreground">{t("auditLog.metadataLimit", { count: STRUCTURED_VALUE_LIMIT })}</p>}
      <details onToggle={event => setRawOpen(event.currentTarget.open)} className="min-w-0 border-t border-border pt-2">
        <summary className="cursor-pointer rounded py-1 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{t("auditLog.metadataRaw")}</summary>
        {rawOpen && <pre className="max-h-72 min-w-0 max-w-full overflow-auto rounded bg-muted p-2 text-xs leading-relaxed text-foreground">{JSON.stringify(details, null, 2)}</pre>}
      </details>
    </section>
  );
}
