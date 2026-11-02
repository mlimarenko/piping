import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Loader2, AlertCircle, Check, Copy } from "lucide-react";
import FormSection from "@/shared/ui-shared/FormSection";
import {
  getRenderedConfigContext,
  type ConfigContextEntityKind,
} from "@/features/inventory/config-contexts-api";
import { useSetting } from "@/shared/settings/hooks";
import {
  CONFIG_CONTEXT_FORMATS,
  CONFIG_CONTEXT_FORMAT_SETTING_KEY,
  DEFAULT_CONFIG_CONTEXT_FORMAT,
  type ConfigContextFormat,
} from "@/shared/settings/registry";
import { serializeStructured } from "@/shared/lib/structuredFormat";
import { writeBrowserClipboardText } from "@/shared/lib/clipboard";
import { cn } from "@/shared/lib/utils";

interface RenderedConfigContextPanelProps {
  kind: ConfigContextEntityKind;
  entityId: string;
  /**
   * Render the panel's own "Config context" title + hint. Off when the panel is
   * embedded under a section header that already names it (e.g. the inspector),
   * so the title is not duplicated.
   */
  showHeader?: boolean;
}

/** True when the rendered context is effectively empty (no keys). */
function isEmptyObject(value: unknown): boolean {
  return (
    value === null ||
    value === undefined ||
    (typeof value === "object" && !Array.isArray(value) && Object.keys(value).length === 0)
  );
}

function FormatSwitcher({
  value,
  onChange,
  label,
}: {
  value: ConfigContextFormat;
  onChange: (next: ConfigContextFormat) => void;
  label: string;
}) {
  return (
    <div
      role="group"
      aria-label={label}
      className="inline-flex shrink-0 overflow-hidden rounded-md border border-border"
    >
      {CONFIG_CONTEXT_FORMATS.map((format) => {
        const active = format === value;
        return (
          <button
            key={format}
            type="button"
            aria-pressed={active}
            onClick={() => onChange(format)}
            className={cn(
              "min-h-8 px-2 text-sm font-medium transition-colors",
              "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
              active
                ? "bg-primary/15 text-primary"
                : "text-muted-foreground hover:bg-accent/20 hover:text-foreground",
            )}
          >
            {format}
          </button>
        );
      })}
    </div>
  );
}

/**
 * Read-only "Config context" panel for a device/VM detail surface. Shows the
 * RENDERED merged context — inherited config contexts overlaid with this
 * entity's `local_context_data` (local wins, NetBox semantics) — plus the
 * contributing contexts in resolution order when present. The display format
 * (JSON / YAML / TOML) follows the effective `inventory.config_context.format`
 * setting (instance default + per-user override; default JSON) and the inline
 * switcher writes the user override.
 */
const RenderedConfigContextPanel = ({
  kind,
  entityId,
  showHeader = true,
}: RenderedConfigContextPanelProps) => {
  const { t } = useTranslation();
  const format = useSetting<ConfigContextFormat>(
    CONFIG_CONTEXT_FORMAT_SETTING_KEY,
    DEFAULT_CONFIG_CONTEXT_FORMAT,
  );
  const [copied, setCopied] = useState(false);

  const query = useQuery({
    queryKey: ["inventory", "config-context", kind, entityId],
    queryFn: () => getRenderedConfigContext(kind, entityId),
  });

  const serialized = useMemo(
    () => serializeStructured(query.data?.rendered, format.value),
    [query.data, format.value],
  );
  const sources = query.data?.sources ?? [];
  const hasLocal = !isEmptyObject(query.data?.local_context_data);
  const empty = isEmptyObject(query.data?.rendered);

  const copy = () => {
    void writeBrowserClipboardText(serialized.text).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1200);
      },
      () => undefined,
    );
  };

  const CopyIcon = copied ? Check : Copy;

  return (
    <FormSection title={showHeader ? t("configContexts.rendered.section") : undefined} description={showHeader ? t("configContexts.rendered.hint") : undefined} contentClassName="flex flex-col gap-2">

      {query.isPending ? (
        <div className="flex items-center justify-center py-4">
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
        </div>
      ) : query.isError ? (
        <div className="flex items-start gap-2 rounded-lg border border-status-negative/30 bg-status-negative/5 px-3 py-2">
          <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-status-negative" />
          <p className="text-xs text-muted-foreground leading-snug">
            {(query.error as { message?: string })?.message ?? t("unknownError")}
          </p>
        </div>
      ) : (
        <>
          {/* Controls: contributing sources (resolution order) + format switcher. */}
          <div className="flex items-start justify-between gap-2">
            <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
              {sources.map((source) => (
                <span
                  key={source.context_id}
                  className="surface-chip inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-xs text-muted-foreground"
                  title={t("configContexts.rendered.sourceWeight", { weight: source.weight })}
                >
                  {source.context_name}
                  <span className="font-mono tabular-nums text-foreground/45">{source.weight}</span>
                </span>
              ))}
              {hasLocal && (
                <span className="inline-flex items-center gap-1 rounded bg-primary/15 px-1.5 py-0.5 text-xs font-medium text-primary">
                  {t("configContexts.rendered.localLayer")}
                </span>
              )}
            </div>
            <FormatSwitcher
              value={format.value}
              onChange={(next) => format.setValue(next)}
              label={t("configContexts.rendered.formatLabel")}
            />
          </div>

          {empty ? (
            <p className="rounded-lg border border-border bg-accent/10 px-3 py-2 text-xs text-muted-foreground">
              {t("configContexts.rendered.empty")}
            </p>
          ) : (
            <div className="relative">
              <button
                type="button"
                onClick={copy}
                aria-label={t("copy")}
                title={copied ? t("copied") : t("copy")}
                className={cn(
                  "absolute right-1.5 top-1.5 inline-flex size-9 items-center justify-center rounded-sm",
                  "text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground",
                  "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                  copied && "text-primary",
                )}
              >
                <CopyIcon className="h-3 w-3" />
              </button>
              <pre className="max-h-[260px] overflow-auto rounded-md border border-border bg-background p-2.5 pr-9 font-mono text-[11px] leading-relaxed text-foreground">
                {serialized.text}
              </pre>
              {serialized.fellBackToJson && (
                <p className="mt-1 text-xs text-muted-foreground">
                  {t("configContexts.rendered.tomlFallback")}
                </p>
              )}
            </div>
          )}
        </>
      )}
    </FormSection>
  );
};

export default RenderedConfigContextPanel;
