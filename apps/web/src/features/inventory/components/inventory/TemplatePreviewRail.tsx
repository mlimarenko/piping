import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, AlertTriangle, Boxes } from "lucide-react";
import FormSection from "@/shared/ui-shared/FormSection";

import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import TemplatePreviewRows from "./TemplatePreviewRows";

import {
  buildTemplatePreview,
  buildTemplateVariableValues,
  formatTemplateValue,
  type InventoryTemplate,
  type PreviewRow,
} from "./templateTypes";
import { previewTemplateRun, type TemplatePreviewStep } from "@/features/inventory/api";

interface Props {
  /** The current (possibly unsaved) draft being authored. */
  template: InventoryTemplate;
  /** Persisted id, if the template already exists (enables server preview). */
  templateId: string | null;
}

type PreviewState =
  | { kind: "local"; rows: PreviewRow[] }
  | { kind: "server"; rows: TemplatePreviewStep[] }
  | { kind: "error"; message: string }
  | { kind: "loading" };

const DEBOUNCE_MS = 450;

/**
 * Sticky live-preview rail. While the author edits, this resolves the template
 * with its default variable values and shows the payloads that *would* be
 * created — the same "what will be created" view the run wizard surfaces, but
 * continuously. Persisted templates use the server dry-run endpoint
 * (`preview-run`); unsaved drafts fall back to the in-browser resolver so the
 * rail is never empty.
 */
const TemplatePreviewRail = ({ template, templateId }: Props) => {
  const { t } = useTranslation();
  const [serverResult, setServerResult] = useState<{
    key: string;
    state: PreviewState;
  } | null>(null);
  const [retryVersion, setRetryVersion] = useState(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Default variable values feed the resolver so {{tokens}} render meaningfully.
  const defaultValues = useMemo(() => {
    const raw: Record<string, string> = {};
    for (const variable of template.variables) {
      raw[variable.key] = formatTemplateValue(variable.default_value);
    }
    return buildTemplateVariableValues(template.variables, raw).values;
  }, [template.variables]);

  const localRows = useMemo(
    () => buildTemplatePreview(template, defaultValues),
    [template, defaultValues],
  );

  // Serialize the inputs that affect the preview so the effect only re-fires on
  // a meaningful change (not on every keystroke in an unrelated meta field).
  const previewSignature = useMemo(
    () => JSON.stringify({ steps: template.steps, values: defaultValues }),
    [template.steps, defaultValues],
  );

  useEffect(() => {
    if (timer.current) clearTimeout(timer.current);

    // No persisted id → the local rows are derived during render, so there is
    // no server round-trip and no state to seed.
    if (!templateId) return;

    const key = `${templateId}|${previewSignature}|${retryVersion}`;
    let cancelled = false;
    timer.current = setTimeout(() => {
      previewTemplateRun(templateId, defaultValues)
        .then((result) => {
          if (cancelled) return;
          setServerResult({ key, state: { kind: "server", rows: result.steps } });
        })
        .catch(() => {
          if (cancelled) return;
          // Graceful fallback: still show the locally-resolved rows so the rail
          // stays useful even when the server rejects a half-finished draft.
          setServerResult({ key, state: { kind: "error", message: "inventory.templates.run.previewError" } });
        });
    }, DEBOUNCE_MS);

    return () => {
      cancelled = true;
      if (timer.current) clearTimeout(timer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- previewSignature (steps+values) is the deliberate re-fire key; defaultValues is subsumed by it and listing t would refetch on language switch
  }, [previewSignature, templateId, retryVersion]);

  // Server state is keyed by the request it answers, so a changed signature or
  // template reads as loading during render — no synchronous setState in the effect.
  let view: PreviewState;
  if (!templateId) {
    view = { kind: "local", rows: localRows };
  } else if (serverResult?.key === `${templateId}|${previewSignature}|${retryVersion}`) {
    view = serverResult.state;
  } else {
    view = { kind: "loading" };
  }

  let rows: PreviewRow[] = [];
  if (view.kind === "server" || view.kind === "local") {
    rows = view.rows;
  } else if (view.kind === "error") {
    rows = localRows;
  }

  return (
    <aside
      className="flex min-w-0 flex-col gap-4"
      aria-label={t("inventory.templates.editor.previewTitle")}
    >
      <FormSection title={t("inventory.templates.editor.previewTitle")} description={t("inventory.templates.editor.previewSubtitle")} action={<>
        {view.kind === "loading" && (
          <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin text-muted-foreground" />
        )}
        {view.kind !== "loading" && rows.length > 0 && (
          <Badge variant="outline" className="ml-auto h-5 text-xs">
            {t("inventory.templates.editor.willCreateCount", { count: rows.length })}
          </Badge>
        )}
      </>}>

      <div className="flex min-w-0 flex-col gap-2">
        {view.kind === "loading" && <p role="status" className="text-xs text-muted-foreground">{t("loading")}</p>}
        {view.kind === "error" && (
          <div role="alert" className="flex items-start gap-2 rounded-md border border-status-warning/30 bg-status-warning/5 p-2 text-xs text-status-warning">
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <div>
              <p className="font-medium">{t("inventory.templates.editor.previewStale")}</p>
              <p className="text-muted-foreground">{t(view.message)}</p>
              <Button size="sm" variant="outline" onClick={() => setRetryVersion((version) => version + 1)}>{t("inventory.templates.run.retry")}</Button>
            </div>
          </div>
        )}

        {rows.length === 0 && view.kind !== "loading" ? (
          <div className="flex flex-col items-center justify-center gap-2 py-4 text-center">
            <Boxes className="h-9 w-8 text-muted-foreground" />
            <p className="text-xs text-muted-foreground">
              {t("inventory.templates.editor.previewEmpty")}
            </p>
          </div>
        ) : (
          <TemplatePreviewRows rows={rows} />
        )}
      </div>
      </FormSection>
    </aside>
  );
};

export default TemplatePreviewRail;
