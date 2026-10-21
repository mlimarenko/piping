import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useParams, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { Database } from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityDetail from "@/shared/ui-shared/EntityDetail";
import PageState from "@/shared/ui-shared/PageState";
import { Button } from "@/shared/ui-primitives/button";
import { customFieldEntityKind, getResourceWithRaw, type ResourceWithRaw } from "./api";
import { inventoryResourceEditPath, isInventoryEntityKey } from "./resourceLinks";
import { inventoryEntityIsReadOnly, tabConfig } from "./components/inventory/inventoryTypes";
import { inventoryDetailText, useInventoryDetailFields } from "./components/inventory/InventoryResourceDetails";
import { EntityCustomFieldsRead } from "./components/custom-fields/EntityCustomFieldsSection";
import RenderedConfigContextPanel from "./components/config-contexts/RenderedConfigContextPanel";
import InventoryResourceChildren from "./components/inventory/InventoryResourceChildren";
import type { InventoryEntityKey } from "@/shared/api/types";

function DetailBody({ entity, item, backHref }: { entity: InventoryEntityKey; item: ResourceWithRaw; backHref: string }) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const fields = useInventoryDetailFields(entity, item);
  const { resource, raw } = item;
  const customKind = customFieldEntityKind(entity);
  const custom = raw.custom_fields ?? raw.custom_field_data ?? resource.customFields;
  const localContext = raw.local_context_data ?? resource.localContextData;
  return <EntityDetail icon={Database} title={resource.name} description={resource.description} backHref={backHref} fields={fields}
    actions={[{ key: "edit", label: t("edit"), href: inventoryResourceEditPath(entity, resource.id, backHref), allowed: !inventoryEntityIsReadOnly(entity) && can("UPDATE", "INVENTORY") }]}>
    {custom && typeof custom === "object" && !Array.isArray(custom) && <section className="flex flex-col gap-2" aria-label={t("customFields.title")}>
      <h2 className="text-base font-semibold">{t("customFields.title")}</h2>
      <EntityCustomFieldsRead key={`${entity}:${resource.id}`} entityKind={customKind ?? null} entityId={resource.id} customFields={custom as Record<string, unknown>} />
    </section>}
    {localContext != null && <section className="flex flex-col gap-2"><h2 className="text-base font-semibold">{t("configContexts.local.label")}</h2>{inventoryDetailText(localContext)}</section>}
    {(entity === "Devices" || entity === "Virtual Machines") && <RenderedConfigContextPanel kind={entity === "Devices" ? "device" : "virtual-machine"} entityId={resource.id} />}
    <InventoryResourceChildren entity={entity} parent={resource} />
  </EntityDetail>;
}

export default function InventoryResourceDetailPage() {
  const { entity: entityParam, id } = useParams();
  const [params] = useSearchParams();
  const { t } = useTranslation();
  const { can } = usePermissions();
  const entity = isInventoryEntityKey(entityParam) ? entityParam : null;
  const readable = can("READ", "INVENTORY");
  const query = useQuery({ queryKey: ["inventory", "resources", entity, "saved-detail", id],
    queryFn: () => getResourceWithRaw(entity!, id!), enabled: Boolean(entity && id && readable) });
  const requestedReturn = params.get("return");
  let url: URL | null = null;
  try { url = requestedReturn ? new URL(requestedReturn, "https://inventory.invalid") : null; }
  catch { /* Invalid user-supplied return destinations use the owning list. */ }
  const backHref = url?.origin === "https://inventory.invalid" && requestedReturn?.startsWith("/")
    ? `${url.pathname}${url.search}${url.hash}` : `/inventory/resources?${new URLSearchParams({ entity: entity ?? "" })}`;
  const title = query.data?.resource.name ?? (entity
    ? t(`inventory.resources.entities.${entity}.one`, { defaultValue: tabConfig.find(config => config.key === entity)?.singular ?? entity })
    : t("inventory.resources.title"));
  const state = (children: ReactNode) => <PageState title={title} icon={Database} backHref={backHref}>{children}</PageState>;
  if (!readable) return state(<p role="alert">{t("accessDenied.title")}</p>);
  if (!entity || !id || (query.error as { status?: number } | null)?.status === 404 || (query.isSuccess && !query.data)) return state(<div role="alert">{t("inventory.resources.notFound")}</div>);
  if (query.isPending) return state(<p role="status">{t("loading")}</p>);
  if (query.isError) return state(<div role="alert">{t("inventory.resources.form.loadFailed")}<Button onClick={() => void query.refetch()}>{t("retry")}</Button></div>);
  return <DetailBody key={`${entity}:${id}`} entity={entity} item={query.data} backHref={backHref} />;
}
