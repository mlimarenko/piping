import { useLocation, Link, useNavigate } from "react-router";
import { ChevronDown, PanelLeftClose } from "lucide-react";
import { useState, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/shared/lib/utils";
import { navLabelClass, navLabelIconClass } from "@/shared/ui-shared/navLabel";
import PipingLogo from "@/shared/ui-shared/PipingLogo";
import { useBranding } from "@/app/BrandingContext";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/shared/ui-primitives/dropdown-menu";
import { AssistantLauncher } from "@/features/assistant";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { resolveNavigation, visibleNavigation, type NavSection } from "@/app/navigation";

interface AppSidebarProps {
  collapsed: boolean;
  onToggle: () => void;
  onNavigate?: () => void;
}


const AppSidebar = ({ collapsed, onToggle, onNavigate }: AppSidebarProps) => {
  const location = useLocation();
  const navigate = useNavigate();
  const { t } = useTranslation();
  const { appName } = useBranding();
  const permissions = usePermissions();
  const [openSections, setOpenSections] = useState<string[]>([]);

  const visibleSections = useMemo(() => visibleNavigation(permissions), [permissions]);
  const owner = resolveNavigation(location.pathname, location.search);
  const activeSectionKey = owner.section?.labelKey;
  const activeItemPath = visibleSections.flatMap(section => section.items)
    .find(item => item.labelKey === owner.item?.labelKey)?.path;

  // External-store sync: auto-expand the active nav section during render while
  // preserving any sections the user manually toggled open. The open-set is
  // shared between this router sync and user toggles, so it cannot be purely
  // derived.
  const [syncedSectionKey, setSyncedSectionKey] = useState<string | undefined>(undefined);
  if (activeSectionKey !== syncedSectionKey) {
    setSyncedSectionKey(activeSectionKey);
    if (activeSectionKey) {
      setOpenSections(prev =>
        prev.includes(activeSectionKey) ? prev : [...prev, activeSectionKey],
      );
    }
  }

  const toggleSection = (labelKey: string) => {
    setOpenSections(prev =>
      prev.includes(labelKey) ? prev.filter(s => s !== labelKey) : [...prev, labelKey]
    );
  };

  const isActive = (path: string) => path === activeItemPath;

  // Exactly one section is "active" — the owner computed above — so the
  // collapsed icon rail never lights two icons at once.
  const isSectionActive = (section: NavSection) => section.labelKey === activeSectionKey;

  return (
    <aside
      data-slot="app-sidebar"
      className={cn(
        "flex h-full max-w-full shrink-0 flex-col border-r border-sidebar-border bg-sidebar transition-[width] duration-150",
        collapsed ? "w-14" : "w-64"
      )}
    >
      {/* Logo as sidebar toggle */}
      <button
        type="button"
        onClick={onToggle}
        aria-label={t("topBar.toggleSidebar")}
        title={t("topBar.toggleSidebar")}
        className={cn(
          "flex h-14 w-full shrink-0 items-center border-b border-sidebar-border transition-colors hover:bg-sidebar-accent",
          collapsed ? "justify-center px-1" : "justify-between px-4"
        )}
      >
        {collapsed ? (
          <PipingLogo size={22} />
        ) : (
          <div className="flex w-full items-center gap-2">
            <PipingLogo size={22} />
            <span className="text-base font-semibold text-foreground tracking-tight truncate">{appName}</span>
            <PanelLeftClose className="ml-auto size-4 text-muted-foreground" aria-hidden="true" />
          </div>
        )}
      </button>

      {/* Nav */}
      <nav className="flex flex-1 flex-col gap-1 overflow-y-auto p-2" role="navigation" aria-label={t("nav.mainNavigation")}>
        {visibleSections.map(section => {
          const isOpen = openSections.includes(section.labelKey);
          const sectionActive = isSectionActive(section);
          const Icon = section.icon;
          const label = t(section.labelKey);

          // Collapsed: render an icon button that opens a DropdownMenu with sub-items
          if (collapsed) {
            return (
              <DropdownMenu key={section.labelKey}>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    className={cn(
                      "flex size-10 items-center justify-center rounded-md transition-colors duration-150",
                      sectionActive
                        ? "bg-sidebar-primary text-sidebar-primary-foreground"
                        : "text-sidebar-foreground hover:bg-sidebar-accent hover:text-foreground"
                    )}
                    aria-label={label}
                    title={label}
                  >
                    <Icon className="h-4 w-4 shrink-0" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  side="right"
                  align="start"
                  sideOffset={6}
                  className="w-56 p-1 border-border/40"
                >
                  <div className={cn(navLabelClass, "px-2 py-1.5 text-xs font-semibold uppercase tracking-wider text-muted-foreground")} title={label}>
                    {label}
                  </div>
                  <div className="flex flex-col gap-1">
                    {section.items.map(item => {
                      const ItemIcon = item.icon;
                      const active = isActive(item.path);
                      const itemLabel = t(item.labelKey, { defaultValue: item.labelDefault });
                      return (
                        <div key={item.path}>
                          {item.subheaderKey && (
                            <div className={cn(navLabelClass, "px-2 pt-2 pb-0.5 text-xs font-semibold uppercase tracking-wider text-muted-foreground/70")} title={t(item.subheaderKey)}>
                              {t(item.subheaderKey)}
                            </div>
                          )}
                          <button
                            type="button"
                            onClick={() => { void navigate(item.path); onNavigate?.(); }}
                            aria-current={active ? "page" : undefined}
                            className={cn(
                              "flex min-h-9 w-full items-start gap-2 rounded-md px-2 py-2 text-left text-sm transition-colors duration-150",
                              active
                                ? "bg-sidebar-primary text-sidebar-primary-foreground font-semibold"
                                : "text-foreground/90 hover:text-foreground hover:bg-accent/40"
                            )}
                          >
                            <ItemIcon className={cn(navLabelIconClass, "h-3.5 w-3.5")} />
                            <span className={navLabelClass} title={itemLabel}>{itemLabel}</span>
                          </button>
                        </div>
                      );
                    })}
                  </div>
                </DropdownMenuContent>
              </DropdownMenu>
            );
          }

          return (
            <div key={section.labelKey} className="flex flex-col gap-1">
              <button
                type="button"
                onClick={() => toggleSection(section.labelKey)}
                className={cn(
                  "flex min-h-10 w-full items-center gap-2 rounded-md px-2 py-2 text-sm font-medium transition-colors duration-150",
                  sectionActive
                    ? "text-primary"
                    : "text-sidebar-foreground hover:bg-sidebar-accent hover:text-foreground"
                )}
                aria-expanded={isOpen}
              >
                <Icon className="size-4 shrink-0" aria-hidden="true" />
                <span className={cn(navLabelClass, "text-left")} title={label}>{label}</span>
                <ChevronDown className={cn(
                  navLabelIconClass,
                  "size-3.5 transition-transform duration-150",
                  !isOpen && "-rotate-90"
                )} />
              </button>

              {isOpen && (
                <div className="ml-4 flex flex-col gap-1 border-l border-border pl-2">
                  {section.items.map(item => {
                    const ItemIcon = item.icon;
                    const active = isActive(item.path);
                    const itemLabel = t(item.labelKey, { defaultValue: item.labelDefault });
                    return (
                      <div key={item.path}>
                        {item.subheaderKey && (
                          <div className={cn(navLabelClass, "px-2 pb-1 pt-4 text-xs font-medium text-muted-foreground")} title={t(item.subheaderKey)}>
                            {t(item.subheaderKey)}
                          </div>
                        )}
                        <Link
                          to={item.path}
                          onClick={onNavigate}
                          aria-current={active ? "page" : undefined}
                          className={cn(
                            "flex min-h-9 items-start gap-2 rounded-md px-2 py-2 text-sm transition-colors duration-150",
                            active
                              ? "bg-sidebar-primary text-sidebar-primary-foreground font-semibold"
                              : "text-sidebar-foreground hover:bg-sidebar-accent hover:text-foreground"
                          )}
                        >
                          <ItemIcon className={cn(navLabelIconClass, "size-4")} aria-hidden="true" />
                          <span className={navLabelClass} title={itemLabel}>{itemLabel}</span>
                        </Link>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </nav>

      <div className={cn("shrink-0 border-t border-sidebar-border p-2", collapsed && "px-1")}>
        <AssistantLauncher collapsed={collapsed} />
      </div>

    </aside>
  );
};

export default AppSidebar;
