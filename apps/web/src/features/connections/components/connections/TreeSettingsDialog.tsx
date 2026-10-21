import { useTranslation } from "react-i18next";
import type { ElementType } from "react";
import {
  ArrowUp,
  ArrowDown,
  Trash2,
  Plus,
  RotateCcw,
  Terminal,
  Monitor,
  Globe,
  ClipboardList,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/shared/ui-primitives/dialog";
import { Button } from "@/shared/ui-primitives/button";
import { Switch } from "@/shared/ui-primitives/switch";
import { Label } from "@/shared/ui-primitives/label";
import { cn } from "@/shared/lib/utils";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import {
  ALL_GROUPS,
  TREE_QUICK_ACCESS_PROTOCOLS,
  type TreeConfig,
  type TreeGroupKey,
  type TreeQuickAccessConfig,
  type TreeQuickAccessProtocol,
} from "./treeConfig";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  config: TreeConfig;
  autoOrder: TreeGroupKey[];
  setMode: (m: "auto" | "custom", seedOrder?: TreeGroupKey[]) => void;
  addGroup: (k: TreeGroupKey) => void;
  removeGroup: (k: TreeGroupKey) => void;
  moveGroup: (k: TreeGroupKey, dir: "up" | "down") => void;
  setIncludeUnknown: (v: boolean) => void;
  setQuickAccess: (next: TreeQuickAccessConfig) => void;
  reset: () => void;
}

const quickAccessIcons: Record<TreeQuickAccessProtocol, ElementType> = {
  ssh: Terminal,
  rdp: Monitor,
  vnc: Monitor,
  app: Globe,
  playbook: ClipboardList,
};

const TreeSettingsDialog = ({
  open,
  onOpenChange,
  config,
  autoOrder,
  setMode,
  addGroup,
  removeGroup,
  moveGroup,
  setIncludeUnknown,
  setQuickAccess,
  reset,
}: Props) => {
  const { t } = useTranslation();
  const available = ALL_GROUPS.filter((g) => !config.order.includes(g));
  const quickAccess = config.quickAccess;

  const updateQuickAccess = (patch: Partial<TreeQuickAccessConfig>) => {
    setQuickAccess({ ...quickAccess, ...patch });
  };

  const toggleProtocol = (protocol: TreeQuickAccessProtocol) => {
    const hasProtocol = quickAccess.protocols.includes(protocol);
    if (hasProtocol && quickAccess.protocols.length <= 1) return;
    updateQuickAccess({
      protocols: hasProtocol
        ? quickAccess.protocols.filter((item) => item !== protocol)
        : [...quickAccess.protocols, protocol],
    });
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="surface sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("connections.treeSettings.title")}</DialogTitle>
          <DialogDescription>
            {t("connections.treeSettings.description")}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div>
            <Label className="text-xs font-medium text-muted-foreground mb-2 block uppercase tracking-wider">
              {t("connections.treeSettings.mode")}
            </Label>
            <div className="flex gap-2">
              <Button
                size="sm"
                variant={config.mode === "auto" ? "default" : "outline"}
                className="h-9 text-sm flex-1"
                onClick={() => setMode("auto")}
              >
                {t("connections.treeSettings.modeAuto")}
              </Button>
              <Button
                size="sm"
                variant={config.mode === "custom" ? "default" : "outline"}
                className="h-9 text-sm flex-1"
                onClick={() => setMode("custom", config.mode === "auto" ? autoOrder : undefined)}
              >
                {t("connections.treeSettings.modeCustom")}
              </Button>
            </div>
          </div>

          <div className={cn(config.mode === "auto" && "opacity-50 pointer-events-none")}>
            <Label className="text-xs font-medium text-muted-foreground mb-2 block uppercase tracking-wider">
              {t("connections.treeSettings.hierarchy")}
            </Label>
            <div className="space-y-1 mb-2">
              {config.order.length === 0 && (
                <div className="text-xs text-muted-foreground italic py-2 text-center">
                  {t("connections.treeSettings.empty")}
                </div>
              )}
              {config.order.map((key, idx) => (
                <div
                  key={key}
                  className="flex items-center gap-2 px-2 py-1.5 rounded-md surface-inset text-xs"
                >
                  <span className="text-muted-foreground font-mono text-xs w-4">
                    {idx + 1}
                  </span>
                  <span className="flex-1">
                    {t(`connections.treeSettings.groups.${key}`, key)}
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8"
                    disabled={idx === 0}
                    onClick={() => moveGroup(key, "up")}
                    aria-label={t("connections.treeSettings.moveUp")}
                  >
                    <ArrowUp className="h-3 w-3" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8"
                    disabled={idx === config.order.length - 1}
                    onClick={() => moveGroup(key, "down")}
                    aria-label={t("connections.treeSettings.moveDown")}
                  >
                    <ArrowDown className="h-3 w-3" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8 text-muted-foreground hover:text-destructive"
                    onClick={() => removeGroup(key)}
                    aria-label={t("connections.treeSettings.remove")}
                  >
                    <Trash2 className="h-3 w-3" />
                  </Button>
                </div>
              ))}
            </div>

            {available.length > 0 && (
              <SearchableSelect
                value=""
                onValueChange={(v) => addGroup(v)}
                placeholder={t("connections.treeSettings.addLevel")}
                searchPlaceholder={t("search")}
                items={available.map((key) => ({
                  id: key,
                  label: t(`connections.treeSettings.groups.${key}`, key),
                  icon: Plus,
                }))}
                triggerClassName="h-9 text-sm"
              />
            )}
          </div>

          <div className="flex items-center justify-between">
            <Label htmlFor="include-unknown" className="text-xs">
              {t("connections.treeSettings.includeUnknown")}
            </Label>
            <Switch
              id="include-unknown"
              checked={config.includeUnknown}
              onCheckedChange={setIncludeUnknown}
            />
          </div>

          <div className="rounded-md border border-border/35 bg-background p-3">
            <div className="mb-2 flex items-center justify-between gap-3">
              <div>
                <Label htmlFor="tree-quick-access" className="text-xs font-medium">
                  {t("connections.treeSettings.quickAccess.title")}
                </Label>
                <p className="mt-0.5 text-xs leading-snug text-muted-foreground">
                  {t("connections.treeSettings.quickAccess.description")}
                </p>
              </div>
              <Switch
                id="tree-quick-access"
                checked={quickAccess.enabled}
                onCheckedChange={(enabled) => updateQuickAccess({ enabled })}
              />
            </div>

            <div className={cn("space-y-3", !quickAccess.enabled && "opacity-50 pointer-events-none")}>
              <div>
                <Label className="mb-1.5 block text-xs font-medium uppercase tracking-wider text-muted-foreground">
                  {t("connections.treeSettings.quickAccess.protocolsLabel")}
                </Label>
                <div className="grid grid-cols-2 gap-1 sm:grid-cols-5">
                  {TREE_QUICK_ACCESS_PROTOCOLS.map((protocol) => {
                    const Icon = quickAccessIcons[protocol];
                    const selected = quickAccess.protocols.includes(protocol);
                    return (
                      <Button
                        key={protocol}
                        type="button"
                        variant={selected ? "default" : "outline"}
                        size="sm"
                        className="h-9 gap-1 px-2 text-sm"
                        onClick={() => toggleProtocol(protocol)}
                        aria-pressed={selected}
                      >
                        <Icon className="h-3 w-3" />
                        {t(`connections.treeSettings.quickAccess.protocols.${protocol}`)}
                      </Button>
                    );
                  })}
                </div>
              </div>

            </div>
          </div>
        </div>

        <DialogFooter className="gap-2">
          <Button variant="outline" size="sm" className="h-9 text-sm gap-1" onClick={reset}>
            <RotateCcw className="h-3 w-3" />
            {t("connections.treeSettings.reset")}
          </Button>
          <Button size="sm" className="h-9 text-sm" onClick={() => onOpenChange(false)}>
            {t("done")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default TreeSettingsDialog;
