import { PanelLeft } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { useTranslation } from "react-i18next";
import UserMenu from "./UserMenu";
import { usePageChromeHosts } from "@/shared/ui-shared/pageChromeContext";
import { IS_DEMO } from "@/demo/env";

interface TopBarProps {
  onToggleMobileSidebar?: () => void;
  showMobileToggle?: boolean;
}

const TopBar = ({ onToggleMobileSidebar, showMobileToggle }: TopBarProps) => {
  const { t } = useTranslation();
  const { owner, actionsBlocked, setTitleHost, setActionsHost } = usePageChromeHosts();

  return (
    <header data-slot="app-header" className="page-gutters flex min-h-14 shrink-0 flex-wrap items-center gap-2 border-b border-border bg-background py-2 sm:gap-4">
      {/* Left: mobile toggle + title */}
      <div className="flex min-w-0 flex-1 basis-40 items-center gap-2">
        {showMobileToggle && (
          <Button id="mobile-navigation-toggle" variant="ghost" size="icon-sm" className="shrink-0" onClick={onToggleMobileSidebar} aria-label={t("topBar.toggleSidebar")}>
            <PanelLeft className="h-4 w-4" />
          </Button>
        )}
        <div ref={setTitleHost} data-slot="page-title-host" className="min-w-0 flex-1" />
        {!owner && <span role="status" className="text-sm text-muted-foreground">{t("loading")}</span>}
      </div>
      <div ref={setActionsHost} data-slot="page-actions-host" inert={actionsBlocked} aria-hidden={actionsBlocked || undefined} className="order-3 flex min-w-0 max-w-full basis-full flex-wrap items-center gap-2 empty:hidden sm:order-none sm:basis-auto" />

      {/* Right: user */}
      <div className="flex items-center gap-1 shrink-0 ml-auto">
        {IS_DEMO && <span className="mr-2 hidden rounded-md border border-border bg-muted px-2 py-1 text-xs text-muted-foreground sm:inline" title={t("topBar.demoNotice")}>{t("topBar.localDemo")}</span>}
        <UserMenu />
      </div>
    </header>
  );
};

export default TopBar;
