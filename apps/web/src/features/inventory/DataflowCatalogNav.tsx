import { useLocation } from "react-router";
import { useTranslation } from "react-i18next";
import { Boxes, Network, Server, Waypoints } from "lucide-react";
import SectionLinks from "@/shared/ui-shared/SectionLinks";

export const DATAFLOW_CATALOG_ROUTES = {
  flows: "/inventory/dataflows",
  applications: "/inventory/dataflows/applications",
  groups: "/inventory/dataflows/groups",
  addressGroups: "/inventory/dataflows/address-groups",
} as const;

export type DataflowCatalogSection = keyof typeof DATAFLOW_CATALOG_ROUTES;

export function dataflowCatalogSection(pathname: string): DataflowCatalogSection {
  if (pathname === DATAFLOW_CATALOG_ROUTES.applications || pathname.startsWith(`${DATAFLOW_CATALOG_ROUTES.applications}/`)) return "applications";
  if (pathname === DATAFLOW_CATALOG_ROUTES.groups || pathname.startsWith(`${DATAFLOW_CATALOG_ROUTES.groups}/`)) return "groups";
  if (pathname === DATAFLOW_CATALOG_ROUTES.addressGroups || pathname.startsWith(`${DATAFLOW_CATALOG_ROUTES.addressGroups}/`)) return "addressGroups";
  return "flows";
}

/** One catalog navigation for flow CRUD plus the three management catalogs. */
export default function DataflowCatalogNav() {
  const { t } = useTranslation();
  const { pathname } = useLocation();
  const active = dataflowCatalogSection(pathname);
  return <SectionLinks label={t("dataflows.catalogNavLabel")} activeValue={active} items={[
    { value: "flows", label: t("dataflows.catalogFlows"), icon: Waypoints, to: DATAFLOW_CATALOG_ROUTES.flows },
    { value: "applications", label: t("dataflows.catalogApplications"), icon: Boxes, to: DATAFLOW_CATALOG_ROUTES.applications },
    { value: "groups", label: t("dataflows.catalogGroups"), icon: Network, to: DATAFLOW_CATALOG_ROUTES.groups },
    { value: "addressGroups", label: t("dataflows.catalogAddressGroups"), icon: Server, to: DATAFLOW_CATALOG_ROUTES.addressGroups },
  ]} />;
}
