import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { User, Settings, LogOut, Languages, Moon, Sun } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/shared/ui-primitives/dropdown-menu";
import { notify } from "@/shared/lib/notifications";
import { logout as apiLogout, me } from "@/features/auth/api";
import { useSetting, normalizeLanguageSetting } from "@/shared/settings";
import { useTheme } from "@/shared/hooks/theme-context";
import { changeAppLanguage } from "@/shared/i18n";

const UserMenu = () => {
  const { t, i18n } = useTranslation();
  const { resolvedTheme, setTheme } = useTheme();
  const { setValue: setLanguage } = useSetting(
    "app.language", { lng: "en" }, { normalize: normalizeLanguageSetting },
  );
  const isDark = resolvedTheme === "dark";
  const currentLanguage = i18n.resolvedLanguage === "ru" ? "ru" : "en";
  const nextLanguage = currentLanguage === "ru" ? "en" : "ru";
  const languageLabel = t(currentLanguage === "ru" ? "language.russian" : "language.english");
  const nextLanguageLabel = t(nextLanguage === "ru" ? "language.russian" : "language.english");
  const nextThemeLabel = t(isDark ? "userSettings.themeLight" : "userSettings.themeDark");
  const navigate = useNavigate();
  const qc = useQueryClient();

  const authQuery = useQuery({
    queryKey: ["auth", "me"],
    queryFn: me,
    retry: false,
  });

  const logoutMutation = useApiMutation({
    mutationFn: () => apiLogout(),
    onSuccess: () => {
      qc.clear();
      notify("info", t("topBar.signOut"));
      void navigate("/auth/login", { replace: true });
    },
    onError: () => {
      // Even on server error clear local state so the user is not stuck.
      qc.clear();
      void navigate("/auth/login", { replace: true });
    },
  });

  const user = authQuery.data;
  const displayName = user?.username ?? "";
  const email = user?.email ?? "";
  const initials = displayName ? displayName.slice(0, 2).toUpperCase() : "--";

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="size-11 sm:size-8"
          aria-label={t("topBar.userMenu")}
        >
          <div className="w-5 h-5 rounded-full bg-primary/10 flex items-center justify-center">
            <User className="h-3 w-3 text-primary" aria-hidden="true" />
          </div>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        className="w-64"
        sideOffset={6}
      >
        <DropdownMenuLabel className="flex items-center gap-2 py-2">
          <div className="w-8 h-8 rounded-full bg-primary/15 flex items-center justify-center text-primary text-xs font-semibold shrink-0">
            {initials}
          </div>
          <div className="min-w-0">
            <p className="text-xs font-medium text-foreground truncate">{displayName || "—"}</p>
            <p className="text-xs text-muted-foreground truncate">{email}</p>
          </div>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={() => navigate("/user/settings")}
        >
          <Settings aria-hidden="true" />
          {t("topBar.account")}
        </DropdownMenuItem>
        <DropdownMenuItem
          aria-label={`${t("userSettings.language")}: ${languageLabel} → ${nextLanguageLabel}`}
          onSelect={() => {
            setLanguage({ lng: nextLanguage });
            void changeAppLanguage(nextLanguage);
          }}
        >
          <Languages aria-hidden="true" />
          {t("userSettings.language")}
          <span className="ml-auto text-xs text-muted-foreground">{languageLabel}</span>
        </DropdownMenuItem>
        <DropdownMenuItem aria-label={`${t("topBar.toggleTheme")}: ${nextThemeLabel}`} onSelect={() => setTheme(isDark ? "light" : "dark")}>
          {isDark ? <Sun aria-hidden="true" /> : <Moon aria-hidden="true" />}
          {t("topBar.toggleTheme")}
          <span className="ml-auto text-xs text-muted-foreground">{nextThemeLabel}</span>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          onSelect={event => { event.preventDefault(); logoutMutation.mutate(); }}
          variant="destructive"
          disabled={logoutMutation.isPending}
        >
          <LogOut aria-hidden="true" />
          {t("topBar.signOut")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
};

export default UserMenu;
