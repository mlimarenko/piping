import type React from "react";
import { useTranslation } from "react-i18next";
import { Plus } from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import type { PlaybookTargetSetListItem } from "./api";

/** Preview of the selected set, resolved by the owning page or dialog. */
export type TargetSetPreviewState =
  | { kind: "idle" }
  | { kind: "denied" }
  | { kind: "pending" }
  | { kind: "error" }
  | { kind: "unknown" }
  | { kind: "ready"; total: number };

/**
 * One target-set selector for jobs, schedules and assignments: the list entry
 * shows name, mode and resolved host count; the owner passes the selected set's
 * preview and the actions it already authorised, never a permission check.
 */
export function TargetSetPicker({
  id,
  "aria-labelledby": ariaLabelledBy,
  targetSets,
  value,
  onValueChange,
  preview,
  disabled = false,
  placeholder,
  onCreate,
  onBrowse,
}: Readonly<{
  id?: string;
  "aria-labelledby"?: string;
  targetSets: PlaybookTargetSetListItem[];
  value: string;
  onValueChange: (id: string) => void;
  preview?: TargetSetPreviewState;
  disabled?: boolean;
  placeholder?: string;
  onCreate?: () => void;
  onBrowse?: () => void;
}>) {
  const { t } = useTranslation();
  const items = targetSets.map((targetSet) => {
    const mode = t(targetSet.mode === "static" ? "playbooks.launchDraft.targetModeStatic" : "playbooks.launchDraft.targetModeDynamic");
    const count = targetSet.resolved_count == null
      ? t("playbooks.targetSetPicker.unknownHosts")
      : t("playbooks.targetSetPicker.hosts", { count: targetSet.resolved_count });
    return {
      id: targetSet.id,
      label: `${targetSet.name} · ${mode} · ${count}`,
      searchText: targetSet.selector ? JSON.stringify(targetSet.selector) : targetSet.mode,
    };
  });
  let previewBox: React.ReactNode = null;
  if (value && preview && preview.kind !== "idle") {
    let content: React.ReactNode;
    if (preview.kind === "denied") content = t("accessDenied.title");
    else if (preview.kind === "pending") content = t("playbooks.launchDialog.loadingPreview");
    else if (preview.kind === "error") content = <span className="text-status-warning">{t("playbooks.launchDialog.previewFailed")}</span>;
    else if (preview.kind === "unknown") content = t("playbooks.targetSetPicker.unknownHosts");
    else content = t("playbooks.page.dialogs.assignment.resolvedHosts", { count: preview.total });
    previewBox = (
      <div className="rounded-md border border-border bg-background px-3 py-2 text-xs text-muted-foreground">
        {content}
      </div>
    );
  }
  return (
    <div className="space-y-1.5">
      <SearchableSelect
        id={id}
        aria-labelledby={ariaLabelledBy}
        value={value}
        onValueChange={onValueChange}
        disabled={disabled || items.length === 0}
        items={items}
        placeholder={placeholder}
        searchPlaceholder={t("search")}
      />
      {previewBox}
      {onBrowse || onCreate ? (
        <div className="flex flex-wrap items-center gap-1">
          {onBrowse ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="px-2"
              onClick={onBrowse}
              disabled={disabled}
            >
              {t("playbooks.page.targetSets.title")}
            </Button>
          ) : null}
          {onCreate ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="gap-2 px-2 text-primary hover:text-primary"
              onClick={onCreate}
              disabled={disabled}
            >
              <Plus className="h-3 w-3" />
              {t("playbooks.page.actions.newTargetSet")}
            </Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
