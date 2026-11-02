import { useId, useState } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Braces } from "lucide-react";
import { Input } from "@/shared/ui-primitives/input";
import { Switch } from "@/shared/ui-primitives/switch";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import FormField from "@/shared/ui-shared/FormField";
import FormSection from "@/shared/ui-shared/FormSection";
import SecretField from "@/shared/ui-shared/SecretField";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormSelect, { type FormSelectOption } from "@/shared/ui-shared/FormSelect";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui-primitives/dropdown-menu";
import ColorField from "@/shared/forms/ColorField";
import {
  CONNCONF_PROTOCOLS,
  revealRecordCredentials,
  type ConnConfParams,
  type ConnConfProtocol,
  type ConnConfRecord,
  protocolDefaultPort,
} from "./api";
import { paramsForProtocolChange, type RecordDraft } from "./recordDraft";

type VariableEntry = { token: string; label: string };
type VariableGroup = { titleKey: string; entries: readonly VariableEntry[] };

/**
 * Canonical source: apps/api/src/contexts/connections/application/
 * connection_params_schema.rs `KNOWN_TARGET_VARIABLE_PATHS` (plus the dynamic
 * `target.custom_fields.<key>` prefix) — keep in sync. The backend resolves these
 * `{{…}}` tokens against the target inventory entity and its related entities at
 * launch time.
 *
 * `host` and local-command app targets accept only the endpoint placeholders
 * (`primary_ip4`/`primary_ip6`); the rich `target.*` namespace is accepted only
 * in web-link (http/https) app `url` targets. See [`ENDPOINT_VARIABLES`] vs
 * [`TARGET_VARIABLE_GROUPS`].
 */
const ENDPOINT_VARIABLES: readonly VariableEntry[] = [
  { token: "{{primary_ip4}}", label: "{{primary_ip4}}" },
  { token: "{{primary_ip6}}", label: "{{primary_ip6}}" },
];

const TARGET_VARIABLE_GROUPS: readonly VariableGroup[] = [
  {
    titleKey: "variableGroupEndpoint",
    entries: ENDPOINT_VARIABLES,
  },
  {
    titleKey: "variableGroupTarget",
    entries: [
      { token: "{{target.name}}", label: "{{target.name}}" },
      { token: "{{target.primary_ip4}}", label: "{{target.primary_ip4}}" },
      { token: "{{target.primary_ip6}}", label: "{{target.primary_ip6}}" },
    ],
  },
  {
    titleKey: "variableGroupRelations",
    entries: [
      { token: "{{target.site.name}}", label: "{{target.site.name}}" },
      { token: "{{target.site.slug}}", label: "{{target.site.slug}}" },
      { token: "{{target.region.name}}", label: "{{target.region.name}}" },
      { token: "{{target.region.slug}}", label: "{{target.region.slug}}" },
      { token: "{{target.tenant.name}}", label: "{{target.tenant.name}}" },
      { token: "{{target.tenant.slug}}", label: "{{target.tenant.slug}}" },
      { token: "{{target.role.name}}", label: "{{target.role.name}}" },
      { token: "{{target.role.slug}}", label: "{{target.role.slug}}" },
      { token: "{{target.platform.name}}", label: "{{target.platform.name}}" },
      { token: "{{target.platform.slug}}", label: "{{target.platform.slug}}" },
      { token: "{{target.device_type.model}}", label: "{{target.device_type.model}}" },
      { token: "{{target.device_type.slug}}", label: "{{target.device_type.slug}}" },
      { token: "{{target.cluster.name}}", label: "{{target.cluster.name}}" },
    ],
  },
  {
    titleKey: "variableGroupCustomFields",
    // Dynamic prefix: replace <key> with the inventory custom-field key.
    entries: [
      { token: "{{target.custom_fields.<key>}}", label: "{{target.custom_fields.<key>}}" },
    ],
  },
];

/**
 * The `host` field and local-command app targets accept ONLY the endpoint
 * placeholders — the rich `target.*` namespace is web-link-only on the backend.
 */
const HOST_VARIABLE_GROUPS: readonly VariableGroup[] = [
  { titleKey: "variableGroupEndpoint", entries: ENDPOINT_VARIABLES },
];

/**
 * An address/url input with an inset "insert variable" adornment. The trigger
 * is a compact icon button anchored inside the field's right edge (mirroring
 * SecretField's eye button), so the input keeps the full cell width. Selecting
 * a placeholder appends a `{{…}}` token to the current value; free-form text is
 * preserved. The placeholder is resolved on the host at launch time.
 *
 * `groups` controls which placeholders the insert menu offers: the `host` field
 * passes only [`ENDPOINT_VARIABLES`]; the app `url` field passes the full
 * [`TARGET_VARIABLE_GROUPS`].
 */
function VariableField({
  id,
  label,
  value,
  onChange,
  placeholder,
  groups,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (next: string | undefined) => void;
  placeholder?: string;
  groups: readonly VariableGroup[];
}) {
  const { t } = useTranslation();
  const text = value;
  return (
    <div className="relative">
      <Input
        id={id}
        className="h-9 w-full text-sm pr-8"
        aria-label={label}
        value={text}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value.trim() || undefined)}
      />
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={t("connectionConfig.fields.insertVariable")}
            className="absolute right-1.5 top-1/2 -translate-y-1/2 flex items-center justify-center text-muted-foreground/50 hover:text-foreground transition-colors rounded outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <Braces className="h-3.5 w-3.5" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="end"
          className="min-w-[220px] max-h-[320px] overflow-y-auto"
        >
          {groups.map((group, groupIndex) => (
            <div key={group.titleKey}>
              {groupIndex > 0 && <DropdownMenuSeparator />}
              <DropdownMenuLabel className="text-xs uppercase tracking-wider text-muted-foreground/70">
                {t(`connectionConfig.fields.${group.titleKey}`)}
              </DropdownMenuLabel>
              {group.entries.map((entry) => (
                <DropdownMenuItem
                  key={entry.token}
                  className="text-[11px] font-mono"
                  onSelect={() => onChange(`${text}${entry.token}`)}
                >
                  {entry.label}
                </DropdownMenuItem>
              ))}
            </div>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}

function numOrUndef(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : undefined;
}

const UUID_RE = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

/**
 * Heuristic for the secret/literal toggle: a canonical secret reference is a
 * bare secret UUID (resolved at connect time via its secret_usage_binding),
 * whereas a literal credential is any other string.
 */
function looksLikeSecretRef(value: string | undefined): boolean {
  return typeof value === "string" && UUID_RE.test(value.trim());
}

function Field({
  label,
  value,
  onChange,
  placeholder,
  render,
  type = "text",
  required = false,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  render?: (field: { id: string; label: string; onChange: (value: string) => void; value: string }) => ReactNode;
  type?: "text" | "number";
  required?: boolean;
}) {
  const inputId = useId();
  if (render) {
    return (
      <FormField htmlFor={inputId} label={label} required={required}>
        {render({ id: inputId, label, onChange, value })}
      </FormField>
    );
  }
  return (
    <FormField htmlFor={inputId} label={label} required={required}>
      <Input
        id={inputId}
        aria-required={required || undefined}
        type={type}
        className="h-9 text-sm"
        value={value}
        placeholder={placeholder}
        onChange={(event) => onChange(event.target.value)}
      />
    </FormField>
  );
}

/** Map a credential field key to its dotted reveal-endpoint path. */
const CREDENTIAL_PATH: Record<"password" | "private_key", string> = {
  password: "credentials.password",
  private_key: "credentials.private_key",
};


const RecordForm = ({
  draft,
  onChange,
  baseRecords,
  recordId,
}: {
  draft: RecordDraft;
  onChange: (next: RecordDraft) => void;
  /** Candidate template parents (same protocol, excluding self). */
  baseRecords: ConnConfRecord[];
  /** Set only when editing an existing record; enables secret reveal. */
  recordId?: string;
}) => {
  const { t } = useTranslation();
  const { protocol, params } = draft;
  const enabledId = useId();
  const baseId = useId();
  const hostId = useId();
  const uriId = useId();

  // Reveal resolved secret-ref values for the record being edited. Disabled on
  // create (no id) — the form's local literals are authoritative there.
  const revealQuery = useQuery({
    queryKey: ["connection-config", "record-credentials", recordId],
    queryFn: () => revealRecordCredentials(recordId as string),
    enabled: !!recordId,
    staleTime: 30_000,
  });
  const canViewSecrets = revealQuery.data?.can_view_secrets ?? false;
  const revealByPath = new Map(
    (revealQuery.data?.fields ?? []).map((f) => [f.path, f]),
  );

  // Per-field "dirty" tracking. A secret-ref field that the user has NOT edited
  // must save back the ORIGINAL ref UUID (held in the draft), never the resolved
  // plaintext we substitute for display — otherwise the secret would leak into
  // params as a literal. Once the user types, the field is dirty and the draft
  // value (literal or new ref) is authoritative.
  const [dirty, setDirty] = useState<Record<"password" | "private_key", boolean>>({
    password: false,
    private_key: false,
  });
  const markDirty = (key: "password" | "private_key") =>
    setDirty((d) => (d[key] ? d : { ...d, [key]: true }));

  /**
   * Resolve how a secret field should render. The draft always carries the raw
   * value (literal or ref UUID); we only swap in resolved plaintext for DISPLAY
   * when the field is an untouched, viewable secret-ref. `loading` is true while
   * the reveal request is in flight for a ref-shaped value so the UUID is masked
   * rather than shown.
   */
  const secretView = (key: "password" | "private_key") => {
    const raw = params.credentials?.[key];
    const field = revealByPath.get(CREDENTIAL_PATH[key]);
    const isRefByReveal = field?.is_secret_ref ?? false;
    const isRefShaped = isRefByReveal || looksLikeSecretRef(raw);
    // Untouched secret-ref with reveal data: show resolved plaintext for display.
    if (!dirty[key] && isRefByReveal) {
      if (canViewSecrets && typeof field?.value === "string") {
        return { value: field.value, canReveal: true, loading: false };
      }
      // Ref exists but caller can't view it — mask, no eye.
      return { value: raw, canReveal: false, loading: false };
    }
    // Ref-shaped value while reveal is still loading: mask the UUID.
    const loading = !dirty[key] && isRefShaped && !!recordId && revealQuery.isPending;
    return { value: raw, canReveal: true, loading };
  };

  const setParams = (next: ConnConfParams) => onChange({ ...draft, params: next });
  const setProtocol = (next: ConnConfProtocol) => {
    if (next === protocol) return;
    setDirty({ password: false, private_key: false });
    onChange({
      ...draft,
      protocol: next,
      baseRecordId: "",
      params: paramsForProtocolChange(next, params),
    });
  };
  const setCredential = (
    key: "username" | "password" | "private_key",
    next: string | undefined,
  ) => {
    const credentials = { ...(params.credentials ?? {}) };
    if (next === undefined) delete credentials[key];
    else credentials[key] = next;
    setParams({
      ...params,
      credentials: Object.keys(credentials).length ? credentials : undefined,
    });
  };

  const hasUsername = protocol === "ssh" || protocol === "rdp";
  const isApp = protocol === "app";
  const isCctv = protocol === "cctv";

  // Canonical DeviceFamily wire values (apps/api device_management domain).
  const videoFamilyOptions: FormSelectOption[] = [
    { value: "dvrip_sofia", label: t("connectionConfig.fields.familyDvripSofia", "DVRIP / Sofia (XMeye)") },
    { value: "dahua", label: t("connectionConfig.fields.familyDahua", "Dahua") },
    { value: "hikvision", label: t("connectionConfig.fields.familyHikvision", "Hikvision") },
    { value: "onvif", label: t("connectionConfig.fields.familyOnvif", "ONVIF") },
  ];

  // Shared password field — identical for the video and generic (ssh/rdp/vnc)
  // branches, which never render together, so one definition serves both.
  const renderPasswordField = () => {
    const view = secretView("password");
    return (
      <SecretField
        label={t("connectionConfig.fields.password")}
        value={view.value}
        canReveal={view.canReveal}
        loading={view.loading}
        onChange={(next) => {
          markDirty("password");
          setCredential("password", next);
        }}
        placeholder={t("connectionConfig.fields.passwordPlaceholder")}
      />
    );
  };

  return (
    <div className="flex flex-col gap-6">
      <FormSection title={t("connectionConfig.section.identity")} contentClassName="flex flex-col gap-4">
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <Field
            label={t("name")}
            value={draft.name}
            onChange={(value) => onChange({ ...draft, name: value })}
            placeholder={t("connectionConfig.namePlaceholder")}
            required
          />
          <FormSelect label={t("connectionConfig.fields.protocol")} value={protocol} onValueChange={setProtocol}
            options={CONNCONF_PROTOCOLS.map(value => ({ value, label: value.toUpperCase() }))} />
        </div>
        <Field
          label={t("connectionConfig.fields.description")}
          value={draft.description}
          onChange={(value) => onChange({ ...draft, description: value })}
          placeholder={t("connectionConfig.descriptionPlaceholder")}
        />
        <div className="flex items-center gap-3"><Switch id={enabledId} checked={draft.enabled} onCheckedChange={enabled => onChange({ ...draft, enabled })} /><FieldLabel htmlFor={enabledId}>{t("connectionConfig.available")}</FieldLabel></div>
        <details open={Boolean(draft.baseRecordId)} className="border-t border-border pt-4">
          <summary className="text-sm font-medium">{t("connectionConfig.inheritance")}</summary>
          <div className="mt-3">
            <FieldLabel htmlFor={baseId}>
              {t("connectionConfig.fields.baseRecord")}
          </FieldLabel>
          <EntitySelect
            id={baseId}
            aria-label={t("connectionConfig.fields.baseRecord")}
            value={draft.baseRecordId || "__none__"}
            onValueChange={(value) =>
              onChange({ ...draft, baseRecordId: value === "__none__" ? "" : value })
            }
            triggerClassName="h-9 text-sm"
            placeholder={t("connectionConfig.fields.baseRecordNone")}
            items={[
              { id: "__none__", label: t("connectionConfig.fields.baseRecordNone") },
              ...baseRecords.map((record) => ({ id: record.id, label: record.name })),
            ]}
          />
          <p className="text-xs text-muted-foreground mt-1">
            {t("connectionConfig.fields.baseRecordHint")}
          </p>
          </div>
        </details>
      </FormSection>

      <FormSection divider title={t("connectionConfig.section.parameters")} contentClassName="flex flex-col gap-4">
        {isApp && (
          <FormField htmlFor={uriId} label={t("connectionConfig.fields.uri")} help={t("connectionConfig.fields.appUriHint")}>
            <VariableField
              id={uriId}
              label={t("connectionConfig.fields.uri")}
              value={params.url ?? ""}
              placeholder={t("connectionConfig.fields.uriPlaceholder")}
              groups={TARGET_VARIABLE_GROUPS}
              onChange={(next) => setParams({ ...params, url: next })}
            />
          </FormField>
        )}
        {isCctv && (
        <>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <FormField htmlFor={hostId} label={t("connectionConfig.fields.host")} help={t("connectionConfig.fields.variableHint")}>
            <VariableField
              id={hostId}
              label={t("connectionConfig.fields.host")}
              value={params.host ?? ""}
              placeholder={t("connectionConfig.fields.hostPlaceholder")}
              groups={HOST_VARIABLE_GROUPS}
              onChange={(next) => setParams({ ...params, host: next })}
            />
          </FormField>
          <Field
            label={t("connectionConfig.fields.mgmtPort", "Management port")}
            type="number"
            value={params.mgmt_port != null ? String(params.mgmt_port) : ""}
            onChange={(value) => setParams({ ...params, mgmt_port: numOrUndef(value) })}
            placeholder="34567 / 80"
          />
          <Field
            label={t("connectionConfig.fields.channel", "Channel")}
            type="number"
            value={params.channel != null ? String(params.channel) : ""}
            onChange={(value) => setParams({ ...params, channel: numOrUndef(value) })}
            placeholder="0"
          />
        </div>

        <div className="grid grid-cols-1 gap-4 border-t border-border pt-4 sm:grid-cols-2 lg:grid-cols-3">
            <FormSelect
              label={t("connectionConfig.fields.family", "Family")}
              value={params.family ?? ""}
              onValueChange={(value) =>
                setParams({ ...params, family: value || undefined })
              }
              options={videoFamilyOptions}
              triggerClassName="h-9 text-sm"
              placeholder={t("connectionConfig.fields.familyPlaceholder", "Select family")}
              aria-label={t("connectionConfig.fields.family", "Family")}
            />
          <Field
            label={t("connectionConfig.fields.username")}
            value={params.credentials?.username ?? ""}
            onChange={(value) => setCredential("username", value.trim() || undefined)}
            placeholder={t("connectionConfig.fields.usernamePlaceholder")}
            required
          />
          {renderPasswordField()}
        </div>
        </>
        )}
        {!isApp && !isCctv && (
        <>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <FormField htmlFor={hostId} label={t("connectionConfig.fields.host")} help={t("connectionConfig.fields.variableHint")}>
            <VariableField
              id={hostId}
              label={t("connectionConfig.fields.host")}
              value={params.host ?? ""}
              placeholder={t("connectionConfig.fields.hostPlaceholder")}
              groups={HOST_VARIABLE_GROUPS}
              onChange={(next) => setParams({ ...params, host: next })}
            />
          </FormField>
          <Field
            label={t("connectionConfig.fields.port")}
            type="number"
            value={params.port != null ? String(params.port) : ""}
            onChange={(value) => setParams({ ...params, port: numOrUndef(value) })}
            placeholder={String(protocolDefaultPort(protocol))}
          />
          <Field
            label={t("connectionConfig.fields.timeout")}
            type="number"
            value={params.timeout_secs != null ? String(params.timeout_secs) : ""}
            onChange={(value) => setParams({ ...params, timeout_secs: numOrUndef(value) })}
            placeholder="15"
          />
        </div>

        <div className="grid grid-cols-1 gap-4 border-t border-border pt-4 sm:grid-cols-2 lg:grid-cols-3">
          {hasUsername && (
            <Field
              label={t("connectionConfig.fields.username")}
              value={params.credentials?.username ?? ""}
              onChange={(value) =>
                setParams({
                  ...params,
                  credentials: {
                    ...(params.credentials ?? {}),
                    username: value.trim() || undefined,
                  },
                })
              }
              placeholder={t("connectionConfig.fields.usernamePlaceholder")}
            />
          )}
          {protocol === "rdp" && (
            <Field
              label={t("connectionConfig.fields.domain")}
              value={params.credentials?.domain ?? ""}
              onChange={(value) =>
                setParams({
                  ...params,
                  credentials: {
                    ...(params.credentials ?? {}),
                    domain: value.trim() || undefined,
                  },
                })
              }
              placeholder={t("connectionConfig.fields.domainPlaceholder")}
            />
          )}
          {renderPasswordField()}
          {protocol === "ssh" && (() => {
            const view = secretView("private_key");
            return (
              <SecretField
                label={t("connectionConfig.fields.privateKey")}
                value={view.value}
                canReveal={view.canReveal}
                loading={view.loading}
                multiline
                onChange={(next) => {
                  markDirty("private_key");
                  setCredential("private_key", next);
                }}
                placeholder={t("connectionConfig.fields.privateKeyPlaceholder")}
              />
            );
          })()}
        </div>

        {protocol === "rdp" && (
          <div className="grid grid-cols-1 gap-4 border-t border-border pt-4 sm:grid-cols-2 lg:grid-cols-3">
            {(["width", "height", "color_depth"] as const).map((key) => (
              <Field
                key={key}
                label={t(`connectionConfig.fields.${key === "color_depth" ? "colorDepth" : key}`)}
                type="number"
                value={params.options?.[key] != null ? String(params.options[key]) : ""}
                onChange={(value) => {
                  const options = { ...(params.options ?? {}) };
                  const parsed = numOrUndef(value);
                  if (parsed === undefined) delete options[key];
                  else options[key] = parsed;
                  setParams({
                    ...params,
                    options: Object.keys(options).length ? options : undefined,
                  });
                }}
                placeholder={key === "width" ? "1920" : key === "height" ? "1080" : "32"}
              />
            ))}
          </div>
        )}
        </>
        )}
      </FormSection>

      <FormSection divider title={t("connectionConfig.section.presentation")}>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <Field
            label={t("connectionConfig.fields.label")}
            value={params.ui?.label ?? ""}
            onChange={(value) =>
              setParams({
                ...params,
                ui: { ...(params.ui ?? {}), label: value.trim() || undefined },
              })
            }
            placeholder={t("connectionConfig.fields.labelPlaceholder")}
          />
          <Field
            label={t("connectionConfig.fields.color")}
            value={params.ui?.color ?? ""}
            onChange={(value) =>
              setParams({
                ...params,
                ui: { ...(params.ui ?? {}), color: value.trim() || undefined },
              })
            }
            render={(field) => (
              <ColorField
                id={field.id}
                value={field.value}
                onChange={field.onChange}
                ariaLabel={field.label}
              />
            )}
          />
        </div>
      </FormSection>
    </div>
  );
};

export default RecordForm;
