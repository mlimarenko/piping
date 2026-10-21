import { useCallback, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueries, useQuery } from "@tanstack/react-query";
import { ArrowRight, KeyRound } from "lucide-react";
import { listResources } from "@/features/inventory/api";
import FormSection from "@/shared/ui-shared/FormSection";
import FormField from "@/shared/ui-shared/FormField";
import EntityRef from "@/shared/ui-shared/EntityRef";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import { Button } from "@/shared/ui-primitives/button";
import {
  CONNCONF_PROTOCOLS,
  getEffectiveConfig,
  type ConnConfProtocol,
  type EffectiveConnConfigSource,
} from "./api";
import { SCOPE_I18N_KEY } from "./scopeCatalog";
import type { ConnConfScopeType } from "./api";

const UUID_RE = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

/**
 * A canonical secret reference is a bare secret UUID under a `credentials.*`
 * leaf (resolved at connect time via its secret_usage_binding). There is no
 * inline secret marker in the params tree.
 */
function isSecretLeaf(path: string, value: unknown): boolean {
  return (
    path.startsWith("credentials.") &&
    typeof value === "string" &&
    UUID_RE.test(value.trim())
  );
}

/** Flatten a params tree into dotted leaf paths matching backend attribution. */
function flattenLeaves(value: unknown, prefix = ""): { path: string; value: unknown }[] {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return Object.entries(value as Record<string, unknown>).flatMap(([key, child]) =>
      flattenLeaves(child, prefix ? `${prefix}.${key}` : key),
    );
  }
  return [{ path: prefix, value }];
}

function renderLeafValue(path: string, value: unknown, secretLabel: string): string {
  if (isSecretLeaf(path, value)) return secretLabel;
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return String(value);
  }
  return JSON.stringify(value);
}

interface EffectiveLeafRow {
  path: string;
  value: unknown;
  displayValue: string;
  secret: boolean;
  sourceLabel: string;
}

const EffectivePreview = () => {
  const { t } = useTranslation();
  const [protocol, setProtocol] = useState<ConnConfProtocol>("ssh");
  const [deviceId, setDeviceId] = useState("");
  const deviceFieldId = useId();

  const [devicesQuery, vmsQuery] = useQueries({
    queries: [
      {
        queryKey: ["inventory", "connconf-preview", "Devices"],
        queryFn: () => listResources("Devices", { limit: 1000, offset: 0 }),
        staleTime: 60_000,
      },
      {
        queryKey: ["inventory", "connconf-preview", "Virtual Machines"],
        queryFn: () => listResources("Virtual Machines", { limit: 1000, offset: 0 }),
        staleTime: 60_000,
      },
    ],
  });
  const targets = useMemo(
    () => [
      ...(devicesQuery.data?.items ?? []).map((r) => ({ id: r.id, name: r.name, kind: "device" })),
      ...(vmsQuery.data?.items ?? []).map((r) => ({ id: r.id, name: r.name, kind: "vm" })),
    ],
    [devicesQuery.data, vmsQuery.data],
  );

  const effectiveQuery = useQuery({
    queryKey: ["connection-config", "effective", deviceId, protocol],
    queryFn: () => getEffectiveConfig(deviceId, protocol),
    enabled: !!deviceId,
  });
  const effective = effectiveQuery.data;

  const leaves = useMemo(
    () => (effective ? flattenLeaves(effective.params) : []),
    [effective],
  );

  const sourceLabel = useCallback((source: EffectiveConnConfigSource): string => {
    if (source.scope_type) {
      const key = SCOPE_I18N_KEY[source.scope_type as ConnConfScopeType] ?? source.scope_type;
      return `${source.record_name} @ ${t(`connectionConfig.scope.${key}`)}`;
    }
    return source.record_name;
  }, [t]);

  const leafRows: EffectiveLeafRow[] = useMemo(
    () =>
      leaves.map((leaf) => {
        const source = effective?.attribution[leaf.path];
        return {
          path: leaf.path,
          value: leaf.value,
          displayValue: renderLeafValue(
            leaf.path,
            leaf.value,
            t("connectionConfig.preview.secretPresent"),
          ),
          secret: isSecretLeaf(leaf.path, leaf.value),
          sourceLabel: source ? sourceLabel(source) : "",
        };
      }),
    [effective?.attribution, leaves, sourceLabel, t],
  );

  const previewColumns: Column<EffectiveLeafRow>[] = useMemo(
    () => [
      {
        key: "path",
        label: t("connectionConfig.preview.field"),
        className: "font-mono",
        getSortValue: (row) => row.path,
        render: (row) => <span className="font-mono text-[11px] text-foreground">{row.path}</span>,
      },
      {
        key: "value",
        label: t("connectionConfig.preview.value"),
        getSortValue: (row) => row.displayValue,
        render: (row) => (
          <span className="inline-flex items-center gap-1 font-mono text-[11px] text-muted-foreground">
            {row.secret && <KeyRound className="h-3 w-3 text-primary" />}
            {row.displayValue}
          </span>
        ),
      },
      {
        key: "source",
        label: t("connectionConfig.preview.source"),
        getSortValue: (row) => row.sourceLabel,
        render: (row) =>
          row.sourceLabel ? (
            <span className="surface-chip rounded px-1.5 py-0.5 text-xs text-primary">
              {row.sourceLabel}
            </span>
          ) : (
            <span className="text-muted-foreground/60">—</span>
          ),
      },
    ],
    [t],
  );

  return (
    <FormSection title={t("connectionConfig.preview.title")} description={t("connectionConfig.preview.description")} contentClassName="flex flex-col gap-4">

      <div className="flex flex-wrap items-end gap-4 border-b border-border pb-4">
        <FormField className="min-w-0 basis-48 flex-1" htmlFor={deviceFieldId} label={t("connectionConfig.preview.device")}>
          <SearchableSelect
            id={deviceFieldId}
            value={deviceId}
            onValueChange={setDeviceId}
            items={targets.map((target) => ({
              id: target.id,
              label: target.name,
              searchText: `${target.name} ${target.kind}`,
            }))}
            placeholder={t("connectionConfig.preview.pickDevice")}
            searchPlaceholder={t("search")}
            emptyText={t("noResults")}
            aria-label={t("connectionConfig.preview.device")}
            panelPlacement="overlay"
          />
        </FormField>
        <div className="w-28">
          <FormSelect<ConnConfProtocol>
            label={t("connectionConfig.fields.protocol")}
            value={protocol}
            onValueChange={setProtocol}
            options={CONNCONF_PROTOCOLS.map((proto) => ({ value: proto, label: proto.toUpperCase() }))}
            aria-label={t("connectionConfig.fields.protocol")}
          />
        </div>
      </div>

      <div className="flex flex-col gap-4">
        {!deviceId && (
          <p className="text-center text-xs text-muted-foreground py-6">
            {t("connectionConfig.preview.selectDevice")}
          </p>
        )}
        {deviceId && effectiveQuery.isPending && (
          <p className="text-center text-xs text-muted-foreground py-6">
            {t("loading")}
          </p>
        )}
        {deviceId && effectiveQuery.isError && (
          <div className="flex flex-wrap items-center justify-center gap-2 py-6">
          <p role="alert" className="text-sm text-destructive">
            {t("connectionConfig.preview.failed")}
          </p>
            <Button variant="outline" size="sm" onClick={() => { void effectiveQuery.refetch(); }}>{t("retry")}</Button>
          </div>
        )}
        {deviceId && effective && (
          <>
            {leaves.length === 0 ? (
              <p className="text-center text-xs text-muted-foreground py-6">
                {t("connectionConfig.preview.noConfig")}
              </p>
            ) : (
              <DataTable
                tableId="connection-config.effective-preview"
                columns={previewColumns}
                data={leafRows}
                getRowKey={(row) => row.path}
                virtualizeRows={false}
                density="compact"
              />
            )}

            {effective.resolution_chain.length > 0 && (
              <div>
                <h3 className="text-sm font-medium text-foreground mb-1.5">
                  {t("connectionConfig.preview.resolutionChain")}
                </h3>
                <div className="flex items-center gap-1 flex-wrap">
                  {effective.resolution_chain.map((id, index) => (
                    <span key={id} className="flex items-center gap-1">
                      {index > 0 && <ArrowRight className="h-3 w-3 text-muted-foreground/50" />}
                      <span className="surface-chip text-[11px] px-1.5 py-0.5 rounded font-mono text-foreground">
                        <EntityRef name={chainName(effective, id)} id={id} />
                      </span>
                    </span>
                  ))}
                </div>
                <p className="text-xs text-muted-foreground mt-1">
                  {t("connectionConfig.preview.chainHint")}
                </p>
              </div>
            )}
          </>
        )}
      </div>
    </FormSection>
  );
};

/** Resolve a record id in the chain to its name via the attribution sources. */
function chainName(
  effective: { attribution: Record<string, EffectiveConnConfigSource> },
  id: string,
): string | undefined {
  for (const source of Object.values(effective.attribution)) {
    if (source.record_id === id) return source.record_name;
  }
  return undefined;
}

export default EffectivePreview;
