import { useTranslation } from "react-i18next";
import { LayoutDashboard, Palette, Server, Search } from "lucide-react";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { derivePalette, type Hsl } from "@/shared/theme/palette";

interface BrandingPreviewProps {
  readonly theme: "light" | "dark";
  readonly appName: string;
  readonly logoDataUrl: string | null;
  readonly base: Hsl | null;
}

/** Local sample only: neutral and semantic tokens belong to the shared theme. */
export function BrandingPreview({ theme, appName, logoDataUrl, base }: BrandingPreviewProps) {
  const { t } = useTranslation();
  const label = theme === "light"
    ? t("appearance.preview.light", { defaultValue: "Light preview" })
    : t("appearance.preview.dark", { defaultValue: "Dark preview" });
  const inventory = t("appearance.preview.inventory", { defaultValue: "Inventory" });
  return (
    <section
      aria-label={label}
      className={`${theme} min-w-0 overflow-hidden rounded-lg border border-border bg-background text-foreground`}
      style={base ? derivePalette(base)[theme] : undefined}
    >
      <header className="flex items-center justify-between gap-2 border-b border-border bg-card px-4 py-2">
        <h2 className="text-xs font-semibold">{label}</h2>
        <span className="text-xs text-muted-foreground">{t("appearance.preview.sample", { defaultValue: "Sample interface" })}</span>
      </header>
      <div className="flex min-h-64">
        <aside className="flex w-24 shrink-0 flex-col gap-4 border-r border-border bg-sidebar p-2 sm:w-32">
          <div className="flex min-w-0 items-center gap-2 text-sidebar-foreground">
            {logoDataUrl ? <img src={logoDataUrl} alt={t("appearance.logo")} className="size-6 shrink-0 object-contain" /> : <Palette aria-hidden="true" className="size-6 shrink-0 text-primary" />}
            <span className="min-w-0 break-words text-sm font-semibold" title={appName}>{appName}</span>
          </div>
          <nav aria-label={t("appearance.preview.navigation", { defaultValue: "Sample navigation" })} className="flex flex-col gap-1 text-sm">
            <span className="flex flex-wrap items-center gap-2 rounded-md bg-accent p-2 font-medium text-accent-foreground"><Server aria-hidden="true" className="size-4 shrink-0" />{inventory}</span>
            <span className="flex flex-wrap items-center gap-2 p-2 text-muted-foreground"><LayoutDashboard aria-hidden="true" className="size-4 shrink-0" />{t("appearance.preview.overview", { defaultValue: "Overview" })}</span>
          </nav>
        </aside>
        <div className="flex min-w-0 flex-1 flex-col gap-4 p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 className="text-sm font-semibold">{inventory}</h3>
            <Button type="button" size="sm" className="h-auto min-h-9 max-w-full whitespace-normal py-2">{t("appearance.preview.primaryAction", { defaultValue: "Add resource" })}</Button>
          </div>
          <div className="relative">
            <Search aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input readOnly value="" className="pl-9" aria-label={t("appearance.preview.search", { defaultValue: "Search resources" })} placeholder={t("appearance.preview.search", { defaultValue: "Search resources" })} />
          </div>
          <ul aria-label={inventory} className="flex flex-col divide-y divide-border rounded-md border border-border bg-card text-sm">
            <li className="flex flex-wrap items-center justify-between gap-2 px-2 py-2"><span>{t("appearance.preview.resource", { defaultValue: "Sample server" })}</span><Badge variant="status-active" className="text-xs">{t("appearance.preview.active", { defaultValue: "Active" })}</Badge></li>
            <li className="flex flex-wrap items-center justify-between gap-2 px-2 py-2"><span>{t("appearance.preview.backup", { defaultValue: "Backup server" })}</span><Badge variant="status-staging" className="text-xs">{t("appearance.preview.pending", { defaultValue: "Pending" })}</Badge></li>
          </ul>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Badge variant="status-offline" className="text-xs">{t("appearance.preview.offline", { defaultValue: "Offline" })}</Badge>
            <Button type="button" size="sm" variant="outline" disabled className="h-auto min-h-9 max-w-full whitespace-normal py-2">{t("appearance.preview.disabledAction", { defaultValue: "Unavailable" })}</Button>
          </div>
        </div>
      </div>
    </section>
  );
}
