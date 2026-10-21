import { useState } from "react";
import { Link, useLocation } from "react-router";
import { playbookDetailHref } from "./playbookDetailRoutes";
import { useTranslation } from "react-i18next";
import {
  Archive,
  Copy,
  Eye,
  FileCode2,
  History,
  Pencil,
  Play,
  Route,
  Trash2,
} from "lucide-react";

import SurfaceCard, { SurfaceCardContent, SurfaceCardHeader } from "@/shared/ui-shared/SurfaceCard";
import RowActions from "@/shared/ui-shared/RowActions";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import {
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/shared/ui-primitives/dropdown-menu";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type { PlaybookCatalogItem, PlaybookTargetSet } from "@/features/playbooks/api";

import { targetSetLabel } from "./playbookPageFormat";
import { ListState, RiskBadge, StatusBadge } from "./PlaybookPanelUi";

export function CatalogPanel({
  playbooks,
  loading,
  targetSetById,
  onRun,
  onAssign,
  onEdit,
  onVersionHistory,
  onUseAsTemplate,
  onArchive,
  onDelete,
}: Readonly<{
  playbooks: PlaybookCatalogItem[];
  loading: boolean;
  targetSetById: Map<string, PlaybookTargetSet>;
  onRun: (item: PlaybookCatalogItem) => void;
  onAssign: (item: PlaybookCatalogItem) => void;
  onEdit: (item: PlaybookCatalogItem) => void;
  onVersionHistory: (item: PlaybookCatalogItem) => void;
  onUseAsTemplate: (item: PlaybookCatalogItem) => void;
  onArchive: (item: PlaybookCatalogItem) => void;
  onDelete: (item: PlaybookCatalogItem) => void;
}>) {
  const { t } = useTranslation();
  let content: React.ReactNode;
  if (loading) {
    content = <ListState label={t("loading")} />;
  } else if (playbooks.length === 0) {
    content = <ListState label={t("playbooks.page.empty.catalog")} />;
  } else {
    content = playbooks.map((item) => (
      <PlaybookRow
        key={item.playbook.id}
        item={item}
        targetSetById={targetSetById}
        onRun={onRun}
        onAssign={onAssign}
        onEdit={onEdit}
        onVersionHistory={onVersionHistory}
        onUseAsTemplate={onUseAsTemplate}
        onArchive={onArchive}
        onDelete={onDelete}
      />
    ));
  }
  return (
    <SurfaceCard className="min-h-0 overflow-hidden">
      <SurfaceCardHeader
        title={<span className="flex items-center gap-2"><FileCode2 aria-hidden="true" className="size-4 text-primary" />{t("playbooks.page.catalog.title")}</span>}
        description={t("playbooks.page.catalog.description")}
      />
      <SurfaceCardContent className="divide-y divide-border/30 p-0 md:p-0">{content}</SurfaceCardContent>
    </SurfaceCard>
  );
}

export function PlaybookRow({
  item,
  targetSetById,
  onRun,
  onAssign,
  onEdit,
  onVersionHistory,
  onUseAsTemplate,
  onArchive,
  onDelete,
}: Readonly<{
  item: PlaybookCatalogItem;
  targetSetById: Map<string, PlaybookTargetSet>;
  onRun: (item: PlaybookCatalogItem) => void;
  onAssign: (item: PlaybookCatalogItem) => void;
  onEdit: (item: PlaybookCatalogItem) => void;
  onVersionHistory: (item: PlaybookCatalogItem) => void;
  onUseAsTemplate: (item: PlaybookCatalogItem) => void;
  onArchive: (item: PlaybookCatalogItem) => void;
  onDelete: (item: PlaybookCatalogItem) => void;
}>) {
  const { t } = useTranslation();
  // View-but-not-edit: gate catalog row mutations; run maps to EXECUTE.
  const location = useLocation();
  const { can } = usePermissions();
  const canExecute = can("EXECUTE", "PLAYBOOK");
  const canUpdate = can("UPDATE", "PLAYBOOK");
  const canCreate = can("CREATE", "PLAYBOOK");
  const canDelete = can("DELETE", "PLAYBOOK");
  const canAssign = can("CREATE", "PLAYBOOK_ASSIGNMENT");
  const [menuOpen, setMenuOpen] = useState(false);
  const defaultTargetSet = item.playbook.default_target_set_id
    ? targetSetById.get(item.playbook.default_target_set_id)
    : undefined;
  const runnable = item.playbook.status === "active" && Boolean(item.current_version);
  // Git-mirrored playbooks are read-only: Edit becomes View and Archive/Delete
  // are hidden (the backend rejects mutation with 409).
  const isManaged = item.playbook.source_kind !== "in_app";
  const closeMenuThen = (action: () => void) => {
    setMenuOpen(false);
    window.setTimeout(action, 180);
  };
  return (
    <div className="grid gap-3 px-4 py-3 md:grid-cols-[minmax(0,1fr)_auto]">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <Link className="font-medium text-sm text-primary hover:underline" to={playbookDetailHref(item.playbook.id, "versions", new URLSearchParams({ returnTo: location.pathname + location.search }).toString())}>{item.playbook.name}</Link>
          <Badge variant="secondary" className="rounded-md">{item.playbook.slug}</Badge>
          <RiskBadge risk={item.playbook.risk} />
          <StatusBadge status={item.playbook.status} />
        </div>
        <p className="mt-1 line-clamp-2 text-sm text-muted-foreground">
          {item.playbook.description || t("playbooks.page.catalog.noDescription")}
        </p>
        <div className="mt-2 flex flex-wrap gap-2 text-xs text-muted-foreground">
          <span>{t("playbooks.page.fields.runtime")}: {item.playbook.runtime}</span>
          <span>{t("playbooks.page.fields.category")}: {item.playbook.category}</span>
          <span>{t("playbooks.page.fields.version")}: {item.current_version?.version ?? "—"}</span>
          <span>
            {t("playbooks.page.fields.defaultTargetSet")}: {targetSetLabel(defaultTargetSet)}
          </span>
        </div>
      </div>
      <div className="flex items-center gap-1.5">
        {canExecute && (
          <Button
            size="sm"
            className="gap-2"
            disabled={!runnable}
            onClick={() => onRun(item)}
          >
            <Play className="h-3.5 w-3.5" />
            {t("playbooks.page.actions.run")}
          </Button>
        )}
        <RowActions label={`${t("actions")}: ${item.playbook.name}`} open={menuOpen} onOpenChange={setMenuOpen}>
            {canAssign && (
              <DropdownMenuItem
                onSelect={(event) => {
                  event.preventDefault();
                  closeMenuThen(() => onAssign(item));
                }}
              >
                <Route className="h-3.5 w-3.5" />
                {t("playbooks.page.actions.assign")}
              </DropdownMenuItem>
            )}
            {/* Managed playbooks make this a read-only "View script"; only the
                editable variant needs UPDATE. */}
            {(isManaged || canUpdate) && (
              <DropdownMenuItem
                onSelect={(event) => {
                  event.preventDefault();
                  closeMenuThen(() => onEdit(item));
                }}
              >
                {isManaged ? (
                  <>
                    <Eye className="h-3.5 w-3.5" />
                    {t("playbooks.page.actions.viewScript")}
                  </>
                ) : (
                  <>
                    <Pencil className="h-3.5 w-3.5" />
                    {t("edit")}
                  </>
                )}
              </DropdownMenuItem>
            )}
            <DropdownMenuItem
              onSelect={(event) => {
                event.preventDefault();
                closeMenuThen(() => onVersionHistory(item));
              }}
            >
              <History className="h-3.5 w-3.5" />
              {t("playbooks.page.actions.versionHistory")}
            </DropdownMenuItem>
            {canCreate && (
              <DropdownMenuItem
                onSelect={(event) => {
                  event.preventDefault();
                  closeMenuThen(() => onUseAsTemplate(item));
                }}
              >
                <Copy className="h-3.5 w-3.5" />
                {t("playbooks.page.actions.useAsTemplate")}
              </DropdownMenuItem>
            )}
            {!isManaged && (canUpdate || canDelete) && (
              <>
                <DropdownMenuSeparator />
                {canUpdate && (
                  <DropdownMenuItem
                    disabled={item.playbook.status === "archived"}
                    onSelect={(event) => {
                      event.preventDefault();
                      closeMenuThen(() => onArchive(item));
                    }}
                  >
                    <Archive className="h-3.5 w-3.5" />
                    {t("playbooks.page.actions.archive")}
                  </DropdownMenuItem>
                )}
                {canDelete && (
                  <DropdownMenuItem
                    variant="destructive"
                    onSelect={(event) => {
                      event.preventDefault();
                      closeMenuThen(() => onDelete(item));
                    }}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                    {t("delete")}
                  </DropdownMenuItem>
                )}
              </>
            )}
        </RowActions>
      </div>
    </div>
  );
}
