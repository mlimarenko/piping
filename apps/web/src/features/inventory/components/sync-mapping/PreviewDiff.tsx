import { useState, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { diffChars, type Change } from "diff";
import { ClipboardList, RefreshCw, Loader2, AlertTriangle } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { cn } from "@/shared/lib/utils";
import type { EntityMapping, FieldMapping } from "@/shared/api/types";
import { getSampleRecord, previewMapping } from "@/features/inventory/sync-mapping-api";

interface PreviewDiffProps {
  entityMapping: EntityMapping;
  fieldMapping: FieldMapping;
  providerId: string;
}

const DEBOUNCE_MS = 300;

// ── Character-level diff renderer ─────────────────────────────────────────────

function renderCharDiff(from: string, to: string): React.ReactNode {
  const changes: Change[] = diffChars(from, to);
  return changes.map((change, i) => {
    if (change.added) {
      return (
        <mark
          key={i}
          className="bg-status-positive/15 text-status-positive rounded-sm px-0.5"
        >
          {change.value}
        </mark>
      );
    }
    if (change.removed) {
      return (
        <del
          key={i}
          className="bg-status-negative/15 text-status-negative line-through rounded-sm px-0.5"
        >
          {change.value}
        </del>
      );
    }
    return (
      <span key={i} className="text-muted-foreground">
        {change.value}
      </span>
    );
  });
}

const PreviewDiff = ({ entityMapping, fieldMapping, providerId }: PreviewDiffProps) => {
  const { t } = useTranslation();
  const [pasteMode, setPasteMode] = useState(false);
  const [pastedJson, setPastedJson] = useState("");
  const [pasteError, setPasteError] = useState<string | null>(null);
  const [customSample, setCustomSample] = useState<Record<string, unknown> | null>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // ── Fetch live sample ─────────────────────────────────────

  const sampleQuery = useQuery({
    queryKey: ["sync", "sample-record", providerId, entityMapping.source_entity],
    queryFn: () => getSampleRecord(providerId, entityMapping.source_entity),
    enabled: !!providerId,
    staleTime: Infinity,
    retry: false,
  });

  const activeSample = customSample ?? sampleQuery.data ?? null;

  // ── Preview mutation ──────────────────────────────────────

  const previewMutation = useApiMutation({
    mutationFn: (sample: Record<string, unknown>) =>
      previewMapping({ entity_mapping: entityMapping, sample_record: sample }),
  });
  // TanStack Query keeps `mutate` referentially stable across renders, so it is
  // the honest debounce dependency instead of the whole mutation result object.
  const { mutate: previewMutate } = previewMutation;

  useEffect(() => {
    if (!activeSample) return;
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      previewMutate(activeSample);
    }, DEBOUNCE_MS);
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, [fieldMapping, activeSample, previewMutate]);

  // ── Paste JSON ────────────────────────────────────────────

  const handleUsePasted = () => {
    try {
      const parsed = JSON.parse(pastedJson) as Record<string, unknown>;
      setCustomSample(parsed);
      setPasteError(null);
      setPasteMode(false);
    } catch {
      setPasteError(t("sync.mapping.invalidJson", "Invalid JSON — check syntax and try again."));
    }
  };

  // ── Empty state ───────────────────────────────────────────

  if (!sampleQuery.isLoading && !activeSample && !pasteMode) {
    return (
      <div className="flex w-full flex-col items-center justify-center py-12 gap-4">
        <ClipboardList className="h-10 w-10 text-muted-foreground" />
        <div className="text-center space-y-1">
          <p className="text-sm font-medium text-foreground">
            {t("sync.mapping.noSampleAvailable", "No sample record available.")}
          </p>
          <p className="mx-auto max-w-xs text-xs text-muted-foreground text-balance">
            {t(
              "sync.mapping.noSampleDescription",
              "Paste a JSON sample or fetch one from NetBox to preview the mapping.",
            )}
          </p>
        </div>
        <div className="flex gap-2">
          {providerId && (
            <Button
              variant="outline"
              size="sm"
              className="h-9 text-sm gap-1"
              onClick={() => sampleQuery.refetch()}
              disabled={sampleQuery.isFetching}
            >
              <RefreshCw
                className={cn("h-3 w-3", sampleQuery.isFetching && "animate-spin")}
              />
              {t("sync.mapping.fetchFromNetBox", "Fetch from NetBox")}
            </Button>
          )}
          <Button
            variant="outline"
            size="sm"
            className="h-9 text-sm gap-1"
            onClick={() => setPasteMode(true)}
          >
            <ClipboardList className="h-3 w-3" />
            {t("sync.mapping.pasteSampleJson", "Paste JSON")}
          </Button>
        </div>
      </div>
    );
  }

  // ── Paste mode ────────────────────────────────────────────

  if (pasteMode) {
    return (
      <div className="space-y-3">
        <div className="flex items-center justify-between">
          <h4 className="text-xs font-medium text-foreground">
            {t("sync.mapping.pasteSampleJson", "Paste sample JSON")}
          </h4>
          <Button
            variant="ghost"
            size="sm"
            className="h-9 text-sm"
            onClick={() => {
              setPasteMode(false);
              setPasteError(null);
            }}
          >
            {t("cancel", "Cancel")}
          </Button>
        </div>
        <Textarea
          className="font-mono text-xs h-40 border-border resize-none"
          placeholder='{ "name": "sw-01", "status": "active" }'
          value={pastedJson}
          onChange={(e) => setPastedJson(e.target.value)}
        />
        {pasteError && (
          <p role="alert" className="text-xs text-destructive mt-0.5">
            {pasteError}
          </p>
        )}
        <Button
          size="sm"
          className="h-9 text-sm"
          disabled={!pastedJson.trim()}
          onClick={handleUsePasted}
        >
          {t("sync.mapping.useSample", "Use this sample")}
        </Button>
      </div>
    );
  }

  // The preview endpoint returns a record-level result plus blocking errors and
  // non-blocking warnings. There is no per-field reverse value, so the forward
  // value is read from the evaluated result by target field and field-level
  // errors are matched from `errors`/`warnings` by their `path`.
  const previewData = previewMutation.data;
  const fieldError = [...(previewData?.errors ?? []), ...(previewData?.warnings ?? [])].find(
    (diagnostic) =>
      diagnostic.path === fieldMapping.target_field ||
      diagnostic.path === fieldMapping.source_field,
  );

  const sourceRaw = activeSample?.[fieldMapping.source_field];
  const sourceStr = sourceRaw !== undefined ? JSON.stringify(sourceRaw) : "—";
  const forwardRaw: unknown = previewData?.result?.[fieldMapping.target_field];
  const forwardStr = forwardRaw !== undefined ? JSON.stringify(forwardRaw) : null;

  return (
    <div className="space-y-3">
      {/* Toolbar */}
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-xs font-medium text-foreground">
          {t("sync.mapping.previewLabel", "Preview")}
        </span>
        <span className="text-xs text-muted-foreground">
          {entityMapping.source_entity} · {fieldMapping.source_field}
        </span>
        <div className="ml-auto flex gap-1">
          {providerId && (
            <Button
              variant="ghost"
              size="sm"
              className="h-9 text-sm px-2 gap-1"
              onClick={() => sampleQuery.refetch()}
              disabled={sampleQuery.isFetching}
            >
              <RefreshCw className={cn("h-3 w-3", sampleQuery.isFetching && "animate-spin")} />
              {t("sync.mapping.refreshSample", "Refresh sample")}
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="h-9 text-sm px-2"
            onClick={() => setPasteMode(true)}
          >
            {t("sync.mapping.pasteSampleJson", "Paste custom sample")}
          </Button>
        </div>
      </div>

      {/* Preview error banner */}
      {previewMutation.isError && (
        <div className="flex items-center gap-2 px-3 py-2 rounded-lg bg-destructive/10 border border-destructive/20">
          <AlertTriangle className="h-3.5 w-3.5 text-destructive shrink-0" />
          <span className="text-xs text-destructive">
            {t("sync.mapping.previewError", "Preview evaluation failed.")}
          </span>
          <Button
            variant="ghost"
            size="sm"
            className="h-9 text-sm ml-auto"
            onClick={() => activeSample && previewMutation.mutate(activeSample)}
          >
            {t("retry", "Retry")}
          </Button>
        </div>
      )}

      {/* Three-column diff */}
      <div className="grid grid-cols-3 gap-2 relative">
        {previewMutation.isPending && (
          <div className="absolute inset-0 col-start-2 col-span-2 flex items-center justify-center bg-background rounded-lg z-10">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        )}

        {/* Source column */}
        <DiffColumn
          label={t("sync.mapping.sourceColumn", "Source")}
          valueStr={sourceStr}
          variant="unchanged"
        />

        {/* Forward column — character diff against source */}
        <DiffColumn
          label={t("sync.mapping.forwardColumn", "Forward")}
          valueStr={forwardStr}
          variant={fieldError ? "error" : "forward"}
          errorMsg={fieldError?.message}
          sourceStr={sourceStr}
          pending={previewMutation.isPending}
        />

        {/* Reverse column — preview returns no per-field reverse value */}
        <DiffColumn
          label={t("sync.mapping.reverseColumn", "Reverse")}
          valueStr={null}
          variant={fieldMapping.reverse_transform === null ? "readonly" : "unchanged"}
          pending={previewMutation.isPending}
          readOnlyLabel={t("sync.mapping.readOnlyBadge", "← read-only")}
          readOnlyTooltip={t(
            "sync.mapping.readOnlyBadgeTooltip",
            "This field is forward-only. Reverse not authored.",
          )}
        />
      </div>

      {/* aria-live region */}
      <div aria-live="polite" className="sr-only">
        {previewMutation.isSuccess ? t("sync.mapping.previewUpdated", "Preview updated.") : ""}
      </div>
    </div>
  );
};

// ── DiffColumn ────────────────────────────────────────────────────────────────

interface DiffColumnProps {
  label: string;
  valueStr: string | null;
  variant: "unchanged" | "forward" | "reverse" | "readonly" | "error";
  errorMsg?: string;
  sourceStr?: string;
  pending?: boolean;
  readOnlyLabel?: string;
  readOnlyTooltip?: string;
}

const DiffColumn = ({
  label,
  valueStr,
  variant,
  errorMsg,
  sourceStr,
  readOnlyLabel,
  readOnlyTooltip,
}: DiffColumnProps) => {
  const isReadOnly = variant === "readonly";
  const isError = variant === "error" && !!errorMsg;

  const headerBg =
    variant === "forward"
      ? "bg-status-positive/10"
      : variant === "reverse"
        ? "bg-status-positive/5"
        : variant === "error"
          ? "bg-status-negative/10"
          : "bg-muted";

  return (
    <div className="rounded-lg border border-border bg-card overflow-hidden">
      <div className={cn("px-3 py-1.5 border-b border-border", headerBg)}>
        <span className="text-sm font-semibold text-foreground">
          {label}
        </span>
      </div>
      <div className="px-3 py-2 min-h-[3rem]">
        {isReadOnly ? (
          <span
            className="text-xs font-medium bg-muted text-muted-foreground px-1.5 py-0.5 rounded"
            title={readOnlyTooltip}
          >
            {readOnlyLabel}
          </span>
        ) : isError ? (
          <p className="text-xs text-destructive">{errorMsg}</p>
        ) : valueStr === null ? (
          <span className="text-xs text-muted-foreground">—</span>
        ) : variant === "unchanged" ? (
          <span className="text-xs font-mono text-muted-foreground">{valueStr}</span>
        ) : sourceStr && sourceStr !== valueStr ? (
          // Character-level diff
          <span className="text-xs font-mono break-all">
            {renderCharDiff(sourceStr, valueStr)}
          </span>
        ) : (
          // No diff — values match
          <span className="text-xs font-mono text-status-positive">{valueStr}</span>
        )}
      </div>
    </div>
  );
};

export default PreviewDiff;
