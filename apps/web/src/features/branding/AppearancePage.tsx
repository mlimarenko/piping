import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Palette, Upload, X, RotateCcw, Loader2, Check } from "lucide-react";

import { useApiMutation } from "@/shared/api/hooks";
import {
  getBranding,
  updateBranding,
  BRANDING_QUERY_KEY,
  type BrandingTheme,
} from "@/shared/api/branding-api";
import { parseBrandingTheme } from "@/shared/theme/branding";
import {
  hexToHsl,
  hslToHex,
  DEFAULT_BASE,
  PALETTE_PRESETS,
  type Hsl,
} from "@/shared/theme/palette";

import PageChrome from "@/shared/ui-shared/PageChrome";
import FormSection from "@/shared/ui-shared/FormSection";
import PageState from "@/shared/ui-shared/PageState";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { useUnsavedChangesGuard } from "@/shared/ui-shared/useUnsavedChangesGuard";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import type { ApiError } from "@/shared/api/client";
import { BrandingPreview } from "./BrandingPreview";

const MAX_IMAGE_BYTES = 512 * 1024;

type PaletteMode = "preset" | "custom" | "default";

interface DraftState {
  appName: string;
  logoDataUrl: string | null;
  faviconDataUrl: string | null;
  mode: PaletteMode;
  presetId: string;
  customHex: string;
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const { result } = reader;
      resolve(typeof result === "string" ? result : "");
    };
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

/** Resolve the active base color from a draft, or `null` for the stock palette. */
function draftBase(draft: DraftState): Hsl | null {
  if (draft.mode === "default") return null;
  if (draft.mode === "preset") {
    const preset = PALETTE_PRESETS.find((p) => p.id === draft.presetId);
    return preset ? preset.base : null;
  }
  return hexToHsl(draft.customHex);
}

const AppearancePage = () => {
  const { t } = useTranslation();
  const qc = useQueryClient();

  // View-but-not-edit: hide save/reset affordances when the caller lacks UPDATE.
  const { can } = usePermissions();
  const canUpdate = can("UPDATE", "BRANDING");

  const brandingQuery = useQuery({
    queryKey: BRANDING_QUERY_KEY,
    queryFn: getBranding,
  });

  const [draft, setDraft] = useState<DraftState | null>(null);
  const [baseline, setBaseline] = useState<DraftState | null>(null);
  const submittedRef = useRef<DraftState | null>(null);
  const logoInputRef = useRef<HTMLInputElement>(null);
  const faviconInputRef = useRef<HTMLInputElement>(null);
  const uploadRevision = useRef({ logoDataUrl: 0, faviconDataUrl: 0 });
  useEffect(() => () => {
    uploadRevision.current.logoDataUrl += 1;
    uploadRevision.current.faviconDataUrl += 1;
  }, []);

  // Seed the editable draft from authoritative data once it arrives, during
  // render — an effect here would paint one frame with the spinner after data
  // is already available.
  const loaded = brandingQuery.data;
  if (loaded && draft === null) {
    const theme: BrandingTheme | null = parseBrandingTheme(loaded.theme);
    const mode: PaletteMode = theme?.mode ?? "default";
    const base = theme?.base;
    const seeded: DraftState = {
      appName: loaded.app_name,
      logoDataUrl: loaded.logo_data_url ?? null,
      faviconDataUrl: loaded.favicon_data_url ?? null,
      mode,
      presetId: theme?.presetId ?? PALETTE_PRESETS[0]!.id,
      customHex: base ? hslToHex(base) : hslToHex(DEFAULT_BASE),
    };
    setDraft(seeded);
    setBaseline(seeded);
  }

  const saveMutation = useApiMutation({
    mutationFn: updateBranding,
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: BRANDING_QUERY_KEY });
      setBaseline(submittedRef.current);
      notify("success", t("appearance.saved"));
    },
    onError: (err: ApiError) =>
      notify("error", t("appearance.saveFailed"), { description: err.message }),
  });

  const dirty =
    draft !== null &&
    baseline !== null &&
    (draft.appName !== baseline.appName ||
      draft.logoDataUrl !== baseline.logoDataUrl ||
      draft.faviconDataUrl !== baseline.faviconDataUrl ||
      draft.mode !== baseline.mode ||
      draft.presetId !== baseline.presetId ||
      draft.customHex !== baseline.customHex);
  const { discardDialog } = useUnsavedChangesGuard(dirty);

  if (!draft) {
    return (
      <div className="flex min-w-0 flex-col gap-6">
        <PageChrome icon={Palette} title={t("appearance.title")} description={t("appearance.description")} />
        {brandingQuery.isError ? <div role="alert">
          <EmptyState icon={Palette} title={t("error")} actionLabel={t("retry")} actionIcon={RotateCcw} onAction={() => { void brandingQuery.refetch(); }} />
        </div> : <PageState kind="loading" title={t("loading")} />}
      </div>
    );
  }

  const patch = (next: Partial<DraftState>) =>
    setDraft((d) => (d ? { ...d, ...next } : d));
  const previewBase = draftBase(draft);
  const invalidCustomColor = draft.mode === "custom" && previewBase === null;

  const handleUpload = async (
    file: File | undefined,
    key: "logoDataUrl" | "faviconDataUrl",
  ) => {
    if (!file || !canUpdate || saveMutation.isPending) return;
    const revision = ++uploadRevision.current[key];
    if (file.size > MAX_IMAGE_BYTES) {
      notify("error", t("appearance.fileTooLarge"));
      return;
    }
    try {
      const dataUrl = await readFileAsDataUrl(file);
      if (revision !== uploadRevision.current[key]) return;
      patch({ [key]: dataUrl });
    } catch {
      notify("error", t("appearance.readFailed"));
    }
  };

  const handleSave = () => {
    if (invalidCustomColor || !canUpdate || saveMutation.isPending) return;
    uploadRevision.current.logoDataUrl += 1;
    uploadRevision.current.faviconDataUrl += 1;
    submittedRef.current = draft;
    const base = draftBase(draft);
    const themeOptions = Object.fromEntries(Object.entries(loaded?.theme ?? {}).filter(([key]) => !["mode", "base", "presetId"].includes(key)));
    const theme: BrandingTheme =
      draft.mode === "default" || !base
        ? { mode: "default" }
        : draft.mode === "preset"
          ? { mode: "preset", presetId: draft.presetId, base }
          : { mode: "custom", base };
    saveMutation.mutate({
      app_name: draft.appName.trim(),
      logo_data_url: draft.logoDataUrl,
      favicon_data_url: draft.faviconDataUrl,
      // BrandingPayload.theme is a free-form JSON column the backend stores
      // opaquely; utoipa emits it as `Record<string, never>`, so the FE-owned
      // BrandingTheme shape needs a structural cast. Fixable only by backend schema work.
      theme: { ...themeOptions, ...theme } as unknown as Record<string, never>,
    });
  };

  const handleReset = () => patch({ mode: "default" });

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageChrome
        icon={Palette}
        title={t("appearance.title")}
        description={t("appearance.description")}
      />

      {dirty && canUpdate && (
        <output className="rounded-md border border-status-warning/40 bg-status-warning/10 px-3 py-2 text-xs text-status-warning">
          {t("editor.unsavedChanges")}
        </output>
      )}

      <fieldset disabled={!canUpdate || saveMutation.isPending} className="grid min-w-0 grid-cols-1 gap-6 lg:grid-cols-2">
        {/* App name */}
        <FormSection>
          <FormField htmlFor="appearance-app-name" label={t("appearance.appName")} help={t("appearance.appNameHint")}>
            <FormFieldControl>
            <Input
              id="appearance-app-name"
              placeholder={t("appearance.appNamePlaceholder")}
              value={draft.appName}
              onChange={(e) => patch({ appName: e.target.value })}
            />
            </FormFieldControl>
          </FormField>
        </FormSection>

        {/* Logo + favicon */}
        <FormSection title={t("appearance.assets")}>
          {/* Stack the pickers on narrow screens: side by side each column is
              too narrow for the choose/clear action row, which then runs under
              the neighbouring column. */}
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <ImageField
              label={t("appearance.logo")}
              value={draft.logoDataUrl}
              inputRef={logoInputRef}
              onPick={() => logoInputRef.current?.click()}
               onClear={() => { uploadRevision.current.logoDataUrl += 1; patch({ logoDataUrl: null }); }}
              onChange={(f) => void handleUpload(f, "logoDataUrl")}
              chooseLabel={t("appearance.choose")}
              clearLabel={t("appearance.clear")}
            />
            <ImageField
              label={t("appearance.favicon")}
              value={draft.faviconDataUrl}
              inputRef={faviconInputRef}
              onPick={() => faviconInputRef.current?.click()}
               onClear={() => { uploadRevision.current.faviconDataUrl += 1; patch({ faviconDataUrl: null }); }}
              onChange={(f) => void handleUpload(f, "faviconDataUrl")}
              chooseLabel={t("appearance.choose")}
              clearLabel={t("appearance.clear")}
            />
          </div>
        </FormSection>

        {/* Palette */}
        <FormSection title={t("appearance.palette")} className="lg:col-span-2">
          <div className="flex flex-col gap-4">
            <div className="grid grid-cols-3 gap-2 sm:grid-cols-4 xl:grid-cols-7">
              {PALETTE_PRESETS.map((preset) => {
                const active = draft.mode === "preset" && draft.presetId === preset.id;
                return (
                  <Button variant="ghost" size="sm"
                    key={preset.id}
                    type="button"
                    aria-pressed={active}
                    onClick={() => patch({ mode: "preset", presetId: preset.id })}
                    className={cn(
                      "h-auto flex-col whitespace-normal gap-2 rounded-lg border bg-card p-2 transition-colors",
                      active
                        ? "border-primary ring-1 ring-primary"
                        : "border-input/30 hover:border-input/60",
                    )}
                  >
                    <span
                      className="h-8 w-8 rounded-full"
                      style={{ backgroundColor: hslToHex(preset.base) }}
                    />
                    <span className="text-sm text-foreground">
                      {t(`appearance.presets.${preset.id}`, preset.label)}
                    </span>
                  </Button>
                );
              })}
            </div>

            <div className="flex flex-wrap items-center gap-3 border-t border-border/30 pt-4">
              <Button variant="ghost" size="sm"
                type="button"
                aria-pressed={draft.mode === "custom"}
                onClick={() => patch({ mode: "custom" })}
                className={cn(
                  "h-9 rounded-md border bg-card px-3 text-sm transition-colors",
                  draft.mode === "custom"
                    ? "border-primary text-primary ring-1 ring-primary"
                    : "border-input/30 text-muted-foreground hover:border-input/60 hover:text-foreground",
                )}
              >
                {t("appearance.custom")}
              </Button>
              <input
                type="color"
                aria-label={t("appearance.customColor")}
                value={hslToHex(hexToHsl(draft.customHex) ?? DEFAULT_BASE)}
                onChange={(e) => patch({ mode: "custom", customHex: e.target.value })}
                className="h-9 w-10 cursor-pointer rounded-md border border-input bg-card p-1"
              />
              <Input
                className="w-28 font-mono"
                aria-label={`${t("appearance.customColor")} (HEX)`}
                aria-invalid={invalidCustomColor}
                aria-describedby="appearance-color-hint"
                value={draft.customHex}
                onChange={(e) => {
                  const v = e.target.value;
                  patch({ mode: "custom", customHex: v });
                }}
                placeholder="#3b82f6"
              />
              <span id="appearance-color-hint" className={cn("text-xs", invalidCustomColor ? "text-destructive" : "text-muted-foreground")}>
                {!invalidCustomColor
                  ? t("appearance.livePreviewActive")
                  : t("appearance.invalidHex")}
              </span>
            </div>
            <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
              <BrandingPreview theme="light" appName={draft.appName} logoDataUrl={draft.logoDataUrl} base={previewBase} />
              <BrandingPreview theme="dark" appName={draft.appName} logoDataUrl={draft.logoDataUrl} base={previewBase} />
            </div>
          </div>
        </FormSection>
      </fieldset>

      {canUpdate && (
        <footer className="mt-6 flex flex-wrap items-center justify-end gap-2 border-t border-border/60 pt-6">
          <Button
            variant="outline"
            disabled={saveMutation.isPending}
            onClick={handleReset}
          >
            <RotateCcw className="h-3 w-3" /> {t("appearance.resetToDefault")}
          </Button>
          <Button
            disabled={saveMutation.isPending || !draft.appName.trim() || invalidCustomColor}
            onClick={handleSave}
          >
            {saveMutation.isPending ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <Check className="h-3 w-3" />
            )}
            {t("save")}
          </Button>
        </footer>
      )}

      {discardDialog}
    </div>
  );
};

interface ImageFieldProps {
  label: string;
  value: string | null;
  inputRef: React.RefObject<HTMLInputElement | null>;
  onPick: () => void;
  onClear: () => void;
  onChange: (file: File | undefined) => void;
  chooseLabel: string;
  clearLabel: string;
}

const ImageField = ({
  label,
  value,
  inputRef,
  onPick,
  onClear,
  onChange,
  chooseLabel,
  clearLabel,
}: ImageFieldProps) => (
  <div className="space-y-2">
    <span className="block text-sm font-medium text-foreground">{label}</span>
    <div className="flex items-center justify-center h-16 w-16 rounded-lg border border-border/30 bg-card overflow-hidden">
      {value ? (
        <img src={value} alt={label} className="h-full w-full object-contain" />
      ) : (
        <Upload className="h-5 w-5 text-muted-foreground" />
      )}
    </div>
    <input
      ref={inputRef}
      type="file"
      accept="image/*"
      className="hidden"
      onChange={(e) => {
        onChange(e.target.files?.[0]);
        e.target.value = "";
      }}
    />
    <div className="flex flex-wrap gap-1.5">
      <Button
        variant="outline"
        size="sm"
        className="h-9 gap-2"
        onClick={onPick}
      >
        <Upload className="h-3 w-3" /> {chooseLabel}
      </Button>
      {value && (
        <Button
          variant="ghost"
          size="sm"
          className="h-9 gap-2"
          onClick={onClear}
        >
          <X className="h-3 w-3" /> {clearLabel}
        </Button>
      )}
    </div>
  </div>
);

export default AppearancePage;
