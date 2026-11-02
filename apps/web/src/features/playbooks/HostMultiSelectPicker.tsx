import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Search, X } from "lucide-react";

import { Input } from "@/shared/ui-primitives/input";
import { Button } from "@/shared/ui-primitives/button";
import { cn } from "@/shared/lib/utils";
import type { ConnectionTarget } from "@/shared/api/types";
import { toSelectedHost, type SelectedHost } from "@/features/playbooks/hostSelection";

interface HostMultiSelectPickerProps {
  targets: ConnectionTarget[];
  selected: SelectedHost[];
  onChange: (next: SelectedHost[]) => void;
  loading?: boolean;
}

/**
 * Inline multi-select host picker for building a static target set.
 *
 * Renders the search box + result checklist INLINE inside the dialog body (no
 * floating popover) so it scrolls with the wheel — mirrors the EntityTargetPicker
 * / WebhookEventPicker pattern. Searches the already-loaded connection inventory
 * client-side so selection is instant and never drops a host between keystrokes.
 */
const HostMultiSelectPicker = ({
  targets,
  selected,
  onChange,
  loading = false,
}: HostMultiSelectPickerProps) => {
  const { t } = useTranslation();
  const [term, setTerm] = useState("");
  const [showBlocked, setShowBlocked] = useState(false);

  const selectedIds = useMemo(() => new Set(selected.map((h) => h.id)), [selected]);
  const readyTargets = useMemo(
    () => targets.filter((target) => target.addressBound !== false),
    [targets],
  );
  const blockedTargets = useMemo(
    () => targets.filter((target) => target.addressBound === false),
    [targets],
  );

  const filtered = useMemo(() => {
    const needle = term.trim().toLowerCase();
    const source = showBlocked ? [...readyTargets, ...blockedTargets] : readyTargets;
    const matched = needle
      ? source.filter(
          (target) =>
            target.name.toLowerCase().includes(needle) ||
            (target.primaryIp ?? "").toLowerCase().includes(needle) ||
            (target.role ?? "").toLowerCase().includes(needle) ||
            (target.site ?? "").toLowerCase().includes(needle),
        )
      : source;
    return matched.slice(0, 100);
  }, [blockedTargets, readyTargets, showBlocked, term]);
  const blockedMatchesCount = useMemo(() => {
    const needle = term.trim().toLowerCase();
    if (!needle || showBlocked) return 0;
    return blockedTargets.filter(
      (target) =>
        target.name.toLowerCase().includes(needle) ||
        (target.primaryIp ?? "").toLowerCase().includes(needle) ||
        (target.role ?? "").toLowerCase().includes(needle) ||
        (target.site ?? "").toLowerCase().includes(needle),
    ).length;
  }, [blockedTargets, showBlocked, term]);

  const toggle = (target: ConnectionTarget) => {
    if (target.addressBound === false) return;
    if (selectedIds.has(target.id)) {
      onChange(selected.filter((h) => h.id !== target.id));
    } else {
      onChange([...selected, toSelectedHost(target)]);
    }
  };

  const remove = (id: string) => onChange(selected.filter((h) => h.id !== id));
  const clearAll = () => onChange([]);

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-2 text-xs text-muted-foreground">
          <span>{t("playbooks.page.hostPicker.selectedSummary", { count: selected.length })}</span>
          <span className="rounded bg-status-positive/10 px-1.5 py-0.5 text-status-positive">
            {t("playbooks.page.hostPicker.readySummary", { count: readyTargets.length })}
          </span>
          {blockedTargets.length > 0 ? (
            <span className="rounded-md bg-muted px-2 py-1">
              {t("playbooks.page.hostPicker.noAddressSummary", { count: blockedTargets.length })}
            </span>
          ) : null}
        </div>
        <div className="flex items-center gap-1">
          {blockedTargets.length > 0 ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="px-2 text-muted-foreground hover:text-foreground"
              onClick={() => setShowBlocked((prev) => !prev)}
            >
              {t(
                showBlocked
                  ? "playbooks.page.hostPicker.showReadyOnly"
                  : "playbooks.page.hostPicker.showAll",
              )}
            </Button>
          ) : null}
          {selected.length > 0 && (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="px-2 text-muted-foreground hover:text-foreground"
              onClick={clearAll}
            >
              {t("playbooks.page.hostPicker.clearAll")}
            </Button>
          )}
        </div>
      </div>

      <div className="relative">
        <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={term}
          onChange={(e) => setTerm(e.target.value)}
          placeholder={t("playbooks.page.hostPicker.searchPlaceholder")}
          className="pl-8 pr-10"
        />
        {term && (
          <button
            type="button"
            onClick={() => setTerm("")}
            aria-label={t("playbooks.page.hostPicker.clearSearch")}
            className="absolute right-1 top-1/2 flex size-8 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        )}
      </div>

      <div className="max-h-[240px] space-y-1 overflow-y-auto overscroll-contain rounded-md border border-border bg-background p-2">
        {loading ? (
          <p className="px-1 py-6 text-center text-sm text-muted-foreground">{t("loading")}</p>
        ) : readyTargets.length === 0 && !showBlocked ? (
          <div className="px-3 py-6 text-center">
            <p className="text-sm font-medium text-foreground">
              {t("playbooks.page.hostPicker.noReadyHosts")}
            </p>
            <p className="mt-1 text-xs text-muted-foreground">
              {t("playbooks.page.hostPicker.noReadyHostsDescription")}
            </p>
            {blockedTargets.length > 0 ? (
              <Button
                type="button"
                variant="link"
                size="sm"
                className="mt-2 px-0"
                onClick={() => setShowBlocked(true)}
              >
                {t("playbooks.page.hostPicker.showAll")}
              </Button>
            ) : null}
          </div>
        ) : filtered.length === 0 && blockedMatchesCount > 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-sm text-muted-foreground">
              {t("playbooks.page.hostPicker.noMatchesWithBlocked", {
                count: blockedMatchesCount,
              })}
            </p>
            <Button
              type="button"
              variant="link"
              size="sm"
              className="mt-2 px-0"
              onClick={() => setShowBlocked(true)}
            >
              {t("playbooks.page.hostPicker.showAll")}
            </Button>
          </div>
        ) : filtered.length === 0 ? (
          <p className="px-1 py-6 text-center text-sm text-muted-foreground">
            {t("playbooks.page.hostPicker.noMatches")}
          </p>
        ) : (
          filtered.map((target) => {
            const checked = selectedIds.has(target.id);
            const disabled = target.addressBound === false;
            return (
              <button
                key={target.id}
                type="button"
                aria-pressed={checked}
                disabled={disabled}
                onClick={() => toggle(target)}
                className={cn(
                  "flex min-h-9 w-full items-center gap-2 rounded-md px-2 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                  disabled
                    ? "cursor-not-allowed opacity-45"
                    : checked
                      ? "bg-accent"
                      : "hover:bg-accent",
                )}
              >
                <span
                  className={cn(
                    "flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-[4px] border",
                    checked
                      ? "border-primary bg-primary text-primary-foreground"
                      : "border-input/30",
                  )}
                >
                  {checked && <Check className="h-2.5 w-2.5" />}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium text-foreground">
                    {target.name}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {disabled
                      ? t("playbooks.page.hostPicker.noAddress")
                      : [target.role, target.site, target.primaryIp].filter(Boolean).join(" · ") || "—"}
                  </span>
                </span>
              </button>
            );
          })
        )}
      </div>

      {selected.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {selected.map((host) => (
            <span
              key={host.id}
              className="inline-flex min-h-9 max-w-[12rem] items-center gap-2 rounded-md border border-border bg-card px-2 py-1 text-xs text-foreground"
            >
              <span className="min-w-0 truncate">{host.name}</span>
              <button
                type="button"
                onClick={() => remove(host.id)}
                className="flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                aria-label={t("playbooks.page.hostPicker.remove")}
              >
                <X className="h-3 w-3" />
              </button>
            </span>
          ))}
        </div>
      )}
    </div>
  );
};

export default HostMultiSelectPicker;
