import { useMemo, useState } from "react";
import { useLocation, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { LucideIcon } from "lucide-react";
import {
  AlertTriangle,
  Bot,
  Database,
  HardDrive,
  KeyRound,
  LayoutDashboard,
  Loader2,
  Monitor,
  Palette,
  RotateCcw,
  Save,
  Settings2,
  SlidersHorizontal,
  Trash2,
} from "lucide-react";

import { cn } from "@/shared/lib/utils";
import { notify } from "@/shared/lib/notifications";
import SurfaceCard, { SurfaceCardHeader, SurfaceCardContent } from "@/shared/ui-shared/SurfaceCard";
import FormSection from "@/shared/ui-shared/FormSection";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Switch } from "@/shared/ui-primitives/switch";
import { Textarea } from "@/shared/ui-primitives/textarea";
import {
  CONNECTION_OPEN_IN_NEW_TAB_PROTOCOLS,
  SERVER_SETTING_DEFINITIONS,
  SETTING_DEFINITIONS,
  normalizeConnectionsOpenInNewTabSetting,
  type ConnectionsOpenInNewTabSettingValue,
  type SettingCategory,
  type SettingDefinition,
  type SettingPersistence,
} from "@/shared/settings/registry";
import {
  readBrowserLocalSetting,
  readSessionSetting,
  removeBrowserLocalSetting,
  removeSessionSetting,
  writeBrowserLocalSetting,
  writeSessionSetting,
} from "@/shared/settings/local-cache";
import {
  useAdminSettingDefaults,
  useSetting,
} from "@/shared/settings/hooks";
import {
  clearSecretField,
  fetchSecretFieldStates,
  setSecretField,
  type SecretFieldStateDto,
  type SettingOverrideDto,
} from "@/shared/settings/api";
import { usePermissions } from "@/shared/auth/permissionsContext";

interface SettingsCatalogProps {
  mode: "user" | "admin";
  className?: string;
  keys?: readonly string[];
  showHeader?: boolean;
  showSectionNav?: boolean;
}

type JsonParseResult =
  | { ok: true; value: unknown }
  | { ok: false; message: string };

const CATEGORY_ORDER: SettingCategory[] = [
  "appearance",
  "connections",
  "workspace",
  "assistant",
  "data",
];

const PERSISTENCE_ICON: Record<SettingPersistence, typeof Database> = {
  server: Database,
  browser: HardDrive,
  session: SlidersHorizontal,
};

const CATEGORY_ICON: Record<SettingCategory, LucideIcon> = {
  appearance: Palette,
  connections: Monitor,
  workspace: LayoutDashboard,
  assistant: Bot,
  data: Database,
};

const ASSISTANT_INSTANCE_SECRET_TARGET = {
  consumerType: "assistant_instance",
  consumerId: "00000000-0000-7000-8000-000000000001",
  ownerScopeType: "instance",
  ownerScopeId: "00000000-0000-7000-8000-000000000001",
  fieldKey: "api_key",
} as const;

const ASSISTANT_API_KEY_DEFINITION: SettingDefinition<string> = {
  key: "assistant.instance.api_key",
  titleKey: "settingsRegistry.assistantApiKey.title",
  descriptionKey: "settingsRegistry.assistantApiKey.description",
  category: "assistant",
  persistence: "server",
  cachePolicy: "none",
  defaultValue: "",
  normalize: (value) => (typeof value === "string" ? value : ""),
  control: "text",
  visibility: "admin",
};

function serializeOptionValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}

function selectValueForDefinition(definition: SettingDefinition, value: unknown): string {
  if (definition.key === "app.theme") {
    const mode =
      typeof value === "object" && value !== null && "mode" in value
        ? (value as { mode?: unknown }).mode
        : value;
    return typeof mode === "string" ? mode : "system";
  }
  if (definition.key === "app.language") {
    const lng =
      typeof value === "object" && value !== null && "lng" in value
        ? (value as { lng?: unknown }).lng
        : value;
    return typeof lng === "string" ? lng : "en";
  }
  return serializeOptionValue(value);
}

function settingValueFromSelect(definition: SettingDefinition, value: string): unknown {
  if (definition.key === "app.theme") return { mode: value };
  if (definition.key === "app.language") return { lng: value };
  const option = definition.options?.find((item) => serializeOptionValue(item.value) === value);
  return definition.normalize(option ? option.value : value);
}

function formatJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    if (value === undefined || value === null) return "";
    if (typeof value === "string") return value;
    if (typeof value === "number" || typeof value === "boolean") return String(value);
    return "[unserializable value]";
  }
}

function parseJsonDraft(draft: string): JsonParseResult {
  try {
    return { ok: true, value: JSON.parse(draft) as unknown };
  } catch (error) {
    return {
      ok: false,
      message: error instanceof Error ? error.message : "Invalid JSON",
    };
  }
}

function groupDefinitions(definitions: readonly SettingDefinition[]) {
  return CATEGORY_ORDER.map((category) => ({
    category,
    items: definitions.filter((definition) => definition.category === category),
  })).filter((group) => group.items.length > 0);
}

function countByPersistence(
  definitions: readonly SettingDefinition[],
  persistence: SettingPersistence,
): number {
  return definitions.filter((definition) => definition.persistence === persistence).length;
}

function stringDraft(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (value === undefined || value === null) return "";
  return formatJson(value);
}

function sourceLabelKey(source: string): string {
  if (source === "user") return "settingsCatalog.sourceUser";
  if (source === "instance") return "settingsCatalog.sourceInstance";
  if (source === "browser") return "settingsCatalog.sourceBrowser";
  if (source === "session") return "settingsCatalog.sourceSession";
  return "settingsCatalog.sourceDefault";
}

function JsonSettingEditor({
  label,
  value,
  onSave,
  disabled,
  readOnly,
}: {
  label: string;
  value: unknown;
  onSave: (value: unknown) => void;
  disabled?: boolean;
  readOnly?: boolean;
}) {
  const { t } = useTranslation();
  const formatted = useMemo(() => formatJson(value), [value]);
  const [draft, setDraft] = useState(formatted);
  const [error, setError] = useState<string | null>(null);

  const dirty = draft !== formatted;

  return (
    <div className="space-y-2">
      <Textarea
        aria-label={label}
        value={draft}
        onChange={(event) => {
          setDraft(event.target.value);
          setError(null);
        }}
        className="min-h-24 resize-y font-mono text-sm"
        spellCheck={false}
        disabled={disabled || readOnly}
      />
      {!readOnly && (
        <div className="flex items-center justify-between gap-2">
          <p className={cn("min-h-4 text-xs", error ? "text-destructive" : "text-muted-foreground")}>
            {error ?? t("settingsCatalog.jsonHint")}
          </p>
          <Button
            type="button"
            size="sm"
            className="shrink-0 gap-1"
            disabled={disabled || !dirty}
            onClick={() => {
              const parsed = parseJsonDraft(draft);
              if (!parsed.ok) {
                setError(parsed.message);
                return;
              }
              onSave(parsed.value);
            }}
          >
            <Save className="h-3 w-3" />
            {t("save")}
          </Button>
        </div>
      )}
    </div>
  );
}

function SelectSettingControl({
  definition,
  value,
  onChange,
  disabled,
}: {
  definition: SettingDefinition;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const currentValue = selectValueForDefinition(definition, value);

  return (
    <FormSelect
      value={currentValue}
      aria-label={t(definition.titleKey)}
      disabled={disabled}
      onValueChange={(next) => onChange(settingValueFromSelect(definition, next))}
      triggerClassName="min-w-40"
      options={(definition.options ?? []).map((option) => ({
        value: serializeOptionValue(option.value),
        label: t(option.labelKey),
      }))}
    />
  );
}

function BooleanSettingControl({
  label,
  value,
  onChange,
  disabled,
}: {
  label: string;
  value: unknown;
  onChange: (value: unknown) => void;
  disabled?: boolean;
}) {
  return (
    <Switch
      aria-label={label}
      checked={Boolean(value)}
      disabled={disabled}
      onCheckedChange={(next) => onChange(next)}
    />
  );
}

function ConnectionsOpenInNewTabControl({
  value,
  onChange,
  disabled,
}: {
  value: unknown;
  onChange: (value: ConnectionsOpenInNewTabSettingValue) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const normalized = normalizeConnectionsOpenInNewTabSetting(value);
  const update = (next: Partial<ConnectionsOpenInNewTabSettingValue>) => {
    onChange(normalizeConnectionsOpenInNewTabSetting({ ...normalized, ...next }));
  };
  const updateProtocol = (protocol: (typeof CONNECTION_OPEN_IN_NEW_TAB_PROTOCOLS)[number], enabled: boolean) => {
    update({
      protocols: {
        ...normalized.protocols,
        [protocol]: enabled,
      },
    });
  };

  return (
    <div className="rounded-md border border-border/30 bg-muted p-2">
      <div className="flex items-start justify-between gap-3 rounded bg-card px-2 py-2">
        <div className="min-w-0">
          <div className="text-sm font-medium text-foreground">
            {t("settingsRegistry.connectionsOpenInNewTab.global")}
          </div>
          <p className="mt-0.5 text-xs leading-4 text-muted-foreground">
            {t("settingsRegistry.connectionsOpenInNewTab.globalHint")}
          </p>
        </div>
        <Switch
          aria-label={t("settingsRegistry.connectionsOpenInNewTab.global")}
          checked={normalized.enabled}
          disabled={disabled}
          onCheckedChange={(next) => update({ enabled: next })}
        />
      </div>
      <div className="mt-2 grid gap-1.5 sm:grid-cols-3">
        {CONNECTION_OPEN_IN_NEW_TAB_PROTOCOLS.map((protocol) => (
          <label
            key={protocol}
            className="flex items-center justify-between gap-2 rounded border border-border/20 bg-card px-2 py-1.5 text-sm"
          >
            <span className="font-medium uppercase text-foreground">{protocol}</span>
            <Switch
              checked={normalized.protocols[protocol]}
              disabled={disabled || normalized.enabled}
              onCheckedChange={(next) => updateProtocol(protocol, next)}
            />
          </label>
        ))}
      </div>
      <p className="mt-2 text-xs leading-4 text-muted-foreground">
        {normalized.enabled
          ? t("settingsRegistry.connectionsOpenInNewTab.protocolsCovered")
          : t("settingsRegistry.connectionsOpenInNewTab.protocolsHint")}
      </p>
    </div>
  );
}

function TextSettingControl({
  label,
  value,
  onSave,
  disabled,
  readOnly,
}: {
  label: string;
  value: unknown;
  onSave: (value: unknown) => void;
  disabled?: boolean;
  readOnly?: boolean;
}) {
  const { t } = useTranslation();
  const formatted = useMemo(() => stringDraft(value), [value]);
  const [draft, setDraft] = useState(formatted);
  const dirty = draft !== formatted;

  return (
    <div className="flex gap-2">
      <Input
        aria-label={label}
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        className="min-w-0"
        disabled={disabled || readOnly}
      />
      {!readOnly && (
        <Button
          type="button"
          size="sm"
          className="shrink-0 gap-1"
          disabled={disabled || !dirty}
          onClick={() => onSave(draft)}
        >
          <Save className="h-3 w-3" />
          {t("save")}
        </Button>
      )}
    </div>
  );
}

function NumberSettingControl({
  definition,
  value,
  onSave,
  disabled,
  readOnly,
}: {
  definition: SettingDefinition;
  value: unknown;
  onSave: (value: unknown) => void;
  disabled?: boolean;
  readOnly?: boolean;
}) {
  const { t } = useTranslation();
  const formatted = useMemo(() => stringDraft(value), [value]);
  const [draft, setDraft] = useState(formatted);
  const dirty = draft !== formatted;

  return (
    <div className="flex gap-2">
      <Input
        aria-label={t(definition.titleKey)}
        type="number"
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        className="min-w-0"
        disabled={disabled || readOnly}
      />
      {!readOnly && (
        <Button
          type="button"
          size="sm"
          className="shrink-0 gap-1"
          disabled={disabled || !dirty}
          onClick={() => onSave(definition.normalize(draft))}
        >
          <Save className="h-3 w-3" />
          {t("save")}
        </Button>
      )}
    </div>
  );
}

function SettingValueEditor({
  definition,
  value,
  onSave,
  disabled,
  readOnly,
}: {
  definition: SettingDefinition;
  value: unknown;
  onSave: (value: unknown) => void;
  disabled?: boolean;
  /** Viewer lacks the mutation capability: show the current value but render
   *  no Save affordance and keep every control inert. */
  readOnly?: boolean;
}) {
  const { t } = useTranslation();
  const inert = disabled || readOnly;
  if (definition.key === "connections.open_in_new_tab") {
    return (
      <ConnectionsOpenInNewTabControl
        value={value}
        onChange={onSave}
        disabled={inert}
      />
    );
  }
  if (definition.control === "select") {
    return (
      <SelectSettingControl
        definition={definition}
        value={value}
        onChange={onSave}
        disabled={inert}
      />
    );
  }
  if (definition.control === "boolean") {
    return (
      <BooleanSettingControl
        label={t(definition.titleKey)}
        value={value}
        onChange={onSave}
        disabled={inert}
      />
    );
  }
  if (definition.control === "text") {
    return (
      <TextSettingControl
        label={t(definition.titleKey)}
        key={`${definition.key}:${stringDraft(value)}`}
        value={value}
        onSave={onSave}
        disabled={disabled}
        readOnly={readOnly}
      />
    );
  }
  if (definition.control === "number") {
    return (
      <NumberSettingControl
        key={`${definition.key}:${stringDraft(value)}`}
        definition={definition}
        value={value}
        onSave={onSave}
        disabled={disabled}
        readOnly={readOnly}
      />
    );
  }
  return (
    <JsonSettingEditor
      label={t(definition.titleKey)}
      key={`${definition.key}:${formatJson(value)}`}
      value={value}
      onSave={onSave}
      disabled={disabled}
      readOnly={readOnly}
    />
  );
}

function SettingRowShell({
  definition,
  source,
  children,
  onReset,
  resetDisabled,
  busy,
}: {
  definition: SettingDefinition;
  source: string;
  children: React.ReactNode;
  onReset?: () => void;
  resetDisabled?: boolean;
  busy?: boolean;
}) {
  const { t } = useTranslation();
  const PersistenceIcon = PERSISTENCE_ICON[definition.persistence];
  const wideControl =
    definition.control === "json" || definition.key === "connections.open_in_new_tab";

  return (
    <div className="min-w-0 border-b border-border/40 py-4 last:border-b-0">
      <div className="flex flex-col gap-3 xl:flex-row xl:items-start xl:justify-between">
        <div className="min-w-0 flex-1 space-y-1.5">
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-semibold text-foreground">
              {t(definition.titleKey)}
            </p>
            <Badge variant="status-neutral" className="h-5 px-1.5 text-xs font-medium">
              {t(sourceLabelKey(source))}
            </Badge>
            <span className="inline-flex items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
              <PersistenceIcon className="h-3 w-3" />
              {t(`settingsCatalog.persistence.${definition.persistence}`)}
            </span>
            {busy && <Loader2 className="h-3 w-3 animate-spin text-muted-foreground" />}
          </div>
          <p className="max-w-4xl text-xs leading-5 text-muted-foreground">
            {t(definition.descriptionKey)}
          </p>
          <code className="block truncate text-xs text-muted-foreground/75">{definition.key}</code>
        </div>

        <div
          className={cn(
            "min-w-0 space-y-2",
            wideControl ? "w-full xl:max-w-[520px]" : "w-full sm:max-w-[360px] xl:w-[360px]",
          )}
        >
          {children}
          {onReset && (
            <div className="flex justify-end">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="gap-1"
                disabled={busy || resetDisabled}
                onClick={onReset}
              >
                <RotateCcw className="h-3 w-3" />
                {t("settingsCatalog.reset")}
              </Button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function ServerUserSettingRow({ definition }: { definition: SettingDefinition }) {
  const { t } = useTranslation();
  const setting = useSetting<unknown>(definition.key, definition.defaultValue, {
    normalize: definition.normalize,
  });

  const saveValue = (value: unknown) => {
    setting.setValue(definition.normalize(value));
    notify("success", t("settingsCatalog.saved"));
  };

  return (
    <SettingRowShell
      definition={definition}
      source={setting.source}
      busy={setting.isFetching}
      onReset={() => {
        setting.resetValue();
        notify("success", t("settingsCatalog.resetDone"));
      }}
      resetDisabled={!setting.hasUserOverride}
    >
      <SettingValueEditor
        definition={definition}
        value={setting.value}
        onSave={saveValue}
        disabled={setting.isLoading}
      />
    </SettingRowShell>
  );
}

function readLocalValue(definition: SettingDefinition): unknown {
  if (definition.persistence === "browser") {
    return readBrowserLocalSetting(definition.key, definition.defaultValue, definition.normalize);
  }
  return readSessionSetting(definition.key, definition.defaultValue, definition.normalize);
}

function writeLocalValue(definition: SettingDefinition, value: unknown): void {
  if (definition.persistence === "browser") {
    writeBrowserLocalSetting(definition.key, value, definition.normalize);
    return;
  }
  writeSessionSetting(definition.key, value, definition.normalize);
}

function removeLocalValue(definition: SettingDefinition): void {
  if (definition.persistence === "browser") {
    removeBrowserLocalSetting(definition.key);
    return;
  }
  removeSessionSetting(definition.key);
}

function LocalUserSettingRow({ definition }: { definition: SettingDefinition }) {
  const { t } = useTranslation();
  const [value, setValue] = useState(() => readLocalValue(definition));

  const saveValue = (next: unknown) => {
    const normalized = definition.normalize(next);
    writeLocalValue(definition, normalized);
    setValue(normalized);
    notify("success", t("settingsCatalog.saved"));
  };

  return (
    <SettingRowShell
      definition={definition}
      source={definition.persistence}
      onReset={() => {
        removeLocalValue(definition);
        setValue(definition.defaultValue);
        notify("success", t("settingsCatalog.resetDone"));
      }}
    >
      <SettingValueEditor definition={definition} value={value} onSave={saveValue} />
    </SettingRowShell>
  );
}

function UserSettingRow({ definition }: { definition: SettingDefinition }) {
  if (definition.persistence === "server") return <ServerUserSettingRow definition={definition} />;
  return <LocalUserSettingRow definition={definition} />;
}

function AdminSettingRow({
  definition,
  override,
  onSave,
  onReset,
  busy,
}: {
  definition: SettingDefinition;
  override: SettingOverrideDto | undefined;
  onSave: (key: string, value: unknown) => void;
  onReset: (key: string) => void;
  busy: boolean;
}) {
  const { can } = usePermissions();
  // Mirrors the backend gate on admin settings mutations (Update × Settings in
  // `ensure_admin_settings_access`): a READ-only viewer sees current values but
  // no Save/Reset affordances that would only 403.
  const canUpdate = can("UPDATE", "SETTINGS");
  const value = override ? definition.normalize(override.value) : definition.defaultValue;
  const saveValue = (next: unknown) => {
    onSave(definition.key, definition.normalize(next));
  };

  return (
    <SettingRowShell
      definition={definition}
      source={override ? "instance" : "unset"}
      busy={busy}
      onReset={
        canUpdate
          ? () => {
              onReset(definition.key);
            }
          : undefined
      }
      resetDisabled={!override}
    >
      <SettingValueEditor
        definition={definition}
        value={value}
        onSave={saveValue}
        disabled={busy}
        readOnly={!canUpdate}
      />
    </SettingRowShell>
  );
}

function AdminAssistantApiKeyRow() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { can } = usePermissions();
  // Same Update × Settings gate as AdminSettingRow: without it the secret
  // Set/Clear affordances are hidden and only the configured/missing state shows.
  const canUpdate = can("UPDATE", "SETTINGS");
  const [draft, setDraft] = useState("");
  const queryKey = [
    "secret-fields",
    ASSISTANT_INSTANCE_SECRET_TARGET.consumerType,
    ASSISTANT_INSTANCE_SECRET_TARGET.consumerId,
  ] as const;
  const stateQuery = useQuery({
    queryKey,
    queryFn: () =>
      fetchSecretFieldStates(
        ASSISTANT_INSTANCE_SECRET_TARGET.consumerType,
        ASSISTANT_INSTANCE_SECRET_TARGET.consumerId,
      ),
    staleTime: 30_000,
  });

  const secretState = stateQuery.data?.find(
    (item: SecretFieldStateDto) => item.field_key === ASSISTANT_INSTANCE_SECRET_TARGET.fieldKey,
  );
  const hasValue = secretState?.has_value ?? false;
  const busy = stateQuery.isFetching;
  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey });
    void queryClient.invalidateQueries({ queryKey: ["assistant"] });
  };
  const saveSecret = useMutation({
    mutationFn: (value: string) => setSecretField(ASSISTANT_INSTANCE_SECRET_TARGET, value),
    onSuccess: () => {
      setDraft("");
      invalidate();
      notify("success", t("settingsCatalog.saved"));
    },
    onError: () => notify("error", t("settingsCatalog.secretSaveFailed")),
  });
  const clearSecret = useMutation({
    mutationFn: () => clearSecretField(ASSISTANT_INSTANCE_SECRET_TARGET),
    onSuccess: () => {
      setDraft("");
      invalidate();
      notify("success", t("settingsCatalog.resetDone"));
    },
    onError: () => notify("error", t("settingsCatalog.secretClearFailed")),
  });
  const isSaving = saveSecret.isPending || clearSecret.isPending;
  const trimmedDraft = draft.trim();

  return (
    <SettingRowShell
      definition={ASSISTANT_API_KEY_DEFINITION}
      source={hasValue ? "instance" : "unset"}
      busy={busy || isSaving}
    >
      <div className="space-y-2">
        {canUpdate && (
          <div className="flex gap-2">
            <div className="relative min-w-0 flex-1">
              <KeyRound className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                aria-label={t(ASSISTANT_API_KEY_DEFINITION.titleKey)}
                type="password"
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                placeholder={t("settingsRegistry.assistantApiKey.placeholder")}
                className="min-w-0 pl-8"
                autoComplete="off"
                disabled={isSaving}
              />
            </div>
            <Button
              type="button"
              size="sm"
              className="shrink-0 gap-1"
              disabled={isSaving || trimmedDraft.length === 0}
              onClick={() => saveSecret.mutate(trimmedDraft)}
            >
              {saveSecret.isPending ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Save className="h-3 w-3" />
              )}
              {t("save")}
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="shrink-0 gap-1 text-destructive hover:text-destructive"
              disabled={isSaving || !hasValue}
              onClick={() => clearSecret.mutate()}
            >
              {clearSecret.isPending ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <Trash2 className="h-3 w-3" />
              )}
              {t("settingsCatalog.clearSecret")}
            </Button>
          </div>
        )}
        <p className="text-xs leading-5 text-muted-foreground">
          {hasValue
            ? t("settingsRegistry.assistantApiKey.configured")
            : t("settingsRegistry.assistantApiKey.missing")}
        </p>
      </div>
    </SettingRowShell>
  );
}

interface SettingsGroupModel {
  category: SettingCategory;
  items: SettingDefinition[];
}

function SettingsSectionNav({
  groups,
  activeCategory,
  location,
}: {
  groups: readonly SettingsGroupModel[];
  activeCategory: SettingCategory;
  location: { pathname: string; search: string; hash: string };
}) {
  const { t } = useTranslation();
  const items = groups.map((group) => {
    const categoryLabel = t(`settingsCatalog.category.${group.category}`);
    const params = new URLSearchParams(location.search);
    if (group.category === groups[0]?.category) params.delete("category");
    else params.set("category", group.category);
    return {
      value: group.category,
      icon: CATEGORY_ICON[group.category],
      label: `${categoryLabel} (${group.items.length})`,
      to: { pathname: location.pathname, search: params.toString() ? `?${params}` : "", hash: location.hash },
    };
  });

  return (
    <SectionLinks items={items} activeValue={activeCategory} label={t("settingsCatalog.sections")} />
  );
}

function SettingsGroup({
  group,
  children,
}: {
  group: SettingsGroupModel;
  children: React.ReactNode;
}) {
  const { t } = useTranslation();
  const CategoryIcon = CATEGORY_ICON[group.category];
  const serverCount = countByPersistence(group.items, "server");
  const localCount = group.items.length - serverCount;

  return (
    <FormSection>
      <SurfaceCardHeader
        className="flex-col items-stretch border-0 p-0 sm:flex-row sm:items-start md:p-0"
        title={<span className="flex items-center gap-2"><CategoryIcon className="size-4 text-primary" />{t(`settingsCatalog.category.${group.category}`)}</span>}
        description={t(`settingsCatalog.categoryDescription.${group.category}`)}
        actions={<div className="flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
          <span className="rounded bg-muted px-2 py-1">{t("settingsCatalog.sectionSettings", { count: group.items.length })}</span>
          {serverCount > 0 && <span className="rounded bg-muted px-2 py-1">{t("settingsCatalog.serverCount", { count: serverCount })}</span>}
          {localCount > 0 && <span className="rounded bg-muted px-2 py-1">{t("settingsCatalog.localCount", { count: localCount })}</span>}
        </div>}
      />
      <SurfaceCardContent className="flex flex-col px-0 pb-0 pt-4 md:px-0 md:pb-0 md:pt-4">{children}</SurfaceCardContent>
    </FormSection>
  );
}

export default function SettingsCatalog({
  mode,
  className,
  keys,
  showHeader = mode === "user",
  showSectionNav = true,
}: SettingsCatalogProps) {
  const { t } = useTranslation();
  const [params] = useSearchParams();
  const location = useLocation();
  const keyFilter = useMemo(() => (keys ? new Set(keys) : null), [keys]);
  const definitions =
    mode === "admin"
      ? SERVER_SETTING_DEFINITIONS.filter((definition) => !keyFilter || keyFilter.has(definition.key))
      : SETTING_DEFINITIONS.filter(
          (definition) =>
            definition.visibility !== "admin" &&
            (!keyFilter || keyFilter.has(definition.key)),
        );
  const groups = useMemo(() => groupDefinitions(definitions), [definitions]);
  // URL-driven category: deep-linkable and refresh-stable like ObservabilityPage's
  // `?tab=` and account settings' `?section=`. Invalid/absent params fall back to
  // the first rendered group.
  const categoryParam = params.get("category");
  const selectedCategory =
    categoryParam && (CATEGORY_ORDER as readonly string[]).includes(categoryParam)
      ? (categoryParam as SettingCategory)
      : null;
  const activeGroup =
    groups.find((group) => group.category === selectedCategory) ?? groups[0] ?? null;
  const adminSettings = useAdminSettingDefaults({ enabled: mode === "admin" });
  const defaultsByKey = useMemo(() => {
    const map = new Map<string, SettingOverrideDto>();
    for (const item of adminSettings.data ?? []) map.set(item.key, item);
    return map;
  }, [adminSettings.data]);

  const serverCount = definitions.filter((definition) => definition.persistence === "server").length;
  const localCount = definitions.filter((definition) => definition.persistence !== "server").length;

  return (
    <SurfaceCard className={cn("overflow-hidden", className)}>
      {showHeader && (
        <SurfaceCardHeader
          title={<span className="flex items-center gap-2"><Settings2 className="size-4 text-primary" />{t(mode === "admin" ? "settingsCatalog.adminTitle" : "settingsCatalog.userTitle")}</span>}
          description={t(mode === "admin" ? "settingsCatalog.adminDescription" : "settingsCatalog.userDescription")}
          actions={<div className="flex items-center gap-2 text-xs text-muted-foreground">
            <span className="bg-muted rounded px-2 py-1">
              {t("settingsCatalog.serverCount", { count: serverCount })}
            </span>
            {mode === "user" && (
              <span className="bg-muted rounded px-2 py-1">
                {t("settingsCatalog.localCount", { count: localCount })}
              </span>
            )}
          </div>}
        />
      )}

      <SurfaceCardContent className="flex flex-col gap-4">
        {mode === "admin" && adminSettings.isError && (
          <div role="alert">
            <EmptyState
              icon={AlertTriangle}
              title={t("error")}
              actionLabel={t("retry")}
              actionIcon={RotateCcw}
              onAction={() => { void adminSettings.refetch(); }}
            />
          </div>
        )}
        {mode === "admin" && adminSettings.isLoading ? (
          <div className="flex items-center justify-center py-8 text-sm text-muted-foreground">
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            {t("loading")}
          </div>
        ) : mode === "admin" && adminSettings.isError && adminSettings.data === undefined ? null
        : activeGroup ? (
          <>
            {showSectionNav && (
              <SettingsSectionNav
                groups={groups}
                activeCategory={activeGroup.category}
                location={location}
              />
            )}
            <SettingsGroup group={activeGroup}>
              {mode === "admin" && activeGroup.category === "assistant" && (
                <AdminAssistantApiKeyRow />
              )}
              {activeGroup.items.map((definition) =>
                mode === "admin" ? (
                  <AdminSettingRow
                    key={definition.key}
                    definition={definition}
                    override={defaultsByKey.get(definition.key)}
                    onSave={(key, value) => adminSettings.upsertDefault({ key, value }, { onSuccess: () => notify("success", t("settingsCatalog.saved")), onError: () => notify("error", t("error")) })}
                    onReset={(key) => adminSettings.deleteDefault(key, { onSuccess: () => notify("success", t("settingsCatalog.resetDone")), onError: () => notify("error", t("error")) })}
                    busy={adminSettings.isSaving}
                  />
                ) : (
                  <UserSettingRow key={definition.key} definition={definition} />
                ),
              )}
            </SettingsGroup>
          </>
        ) : (
          <EmptyState icon={Settings2} title={t("settingsCatalog.empty")} />
        )}
      </SurfaceCardContent>
    </SurfaceCard>
  );
}
