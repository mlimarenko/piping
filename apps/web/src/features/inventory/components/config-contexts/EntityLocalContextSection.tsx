import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertCircle, CheckCircle2 } from "lucide-react";
import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import { Textarea } from "@/shared/ui-primitives/textarea";
import FormSection from "@/shared/ui-shared/FormSection";
import {
  LocalContextValidationError,
  type EntityLocalContextHandle,
} from "@/features/inventory/components/config-contexts/localContextErrors";

interface EntityLocalContextSectionProps {
  /** The entity's stored `local_context_data` (arbitrary JSON), if any. */
  localContextData: unknown;
  /** Imperative handle the host dialog calls on save. */
  onRegister: (handle: EntityLocalContextHandle) => void;
}

function tryPrettyJson(raw: string): string | null {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return null;
  }
}

function isValidJson(raw: string): boolean {
  if (raw.trim() === "") return true;
  try {
    JSON.parse(raw);
    return true;
  } catch {
    return false;
  }
}

/** Seed the editor from the stored value: pretty-printed JSON, or empty. */
function seedFrom(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  return JSON.stringify(value, null, 2);
}

/**
 * A JSON editor for a device/VM's local config context override. Mirrors the
 * custom-fields JSON-field contract: it validates on every keystroke and
 * exposes a {@link buildPayload} the host dialog calls on save (returning
 * `undefined` when untouched so the stored value is preserved, `null` when the
 * operator cleared it). The host gates the save on invalid JSON.
 */
const EntityLocalContextSection = ({
  localContextData,
  onRegister,
}: EntityLocalContextSectionProps) => {
  const { t } = useTranslation();
  const seed = useMemo(() => seedFrom(localContextData), [localContextData]);
  const [value, setValue] = useState<string>(seed);
  const [dirty, setDirty] = useState(false);

  const valid = isValidJson(value);
  const hasContent = value.trim().length > 0;

  const buildPayload = useCallback((): unknown => {
    // Untouched: leave the stored override intact.
    if (!dirty) return undefined;
    const raw = value.trim();
    // Cleared: send null so the backend drops the override.
    if (raw === "") return null;
    try {
      return JSON.parse(raw);
    } catch {
      throw new LocalContextValidationError();
    }
  }, [dirty, value]);

  useEffect(() => {
    onRegister({ buildPayload });
  }, [onRegister, buildPayload]);

  return (
    <FormSection title={t("configContexts.local.label")} description={t("configContexts.local.hint")} action={
        <div className="flex items-center gap-2">
          {!valid ? (
            <span className="inline-flex items-center gap-1 text-xs text-status-negative">
              <AlertCircle className="h-3 w-3" />
              {t("configContexts.invalidJson")}
            </span>
          ) : hasContent ? (
            <CheckCircle2 className="h-3.5 w-3.5 text-status-positive/60" />
          ) : null}
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-9 text-sm text-muted-foreground hover:text-foreground"
            disabled={!valid || !hasContent}
            onClick={() => {
              const pretty = tryPrettyJson(value);
              if (pretty !== null) {
                setValue(pretty);
                setDirty(true);
              }
            }}
          >
            {t("configContexts.formatJson")}
          </Button>
        </div>
      }>
      <Textarea
        className={cn(
          "font-mono text-sm min-h-[120px] resize-y border-border bg-background",
          !valid && "border-status-negative/40 focus-visible:ring-status-negative/40",
        )}
        value={value}
        onChange={(e) => {
          setValue(e.target.value);
          setDirty(true);
        }}
        placeholder={t("configContexts.local.placeholder")}
        spellCheck={false}
        aria-label={t("configContexts.local.label")}
        aria-invalid={!valid}
      />
    </FormSection>
  );
};

export default EntityLocalContextSection;
