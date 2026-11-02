import { Suspense, useCallback, useEffect, useRef, useState } from "react";
import { Outlet, useLocation } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import AppSidebar from "./AppSidebar";
import TopBar from "./TopBar";
import StatusBar from "./StatusBar";
import { useIsMobile } from "@/shared/hooks/use-mobile";
import { me } from "@/features/auth/api";
import { PermissionsProvider } from "@/shared/auth/permissions";
import { AlertCircle, Loader2 } from "lucide-react";
import { ApiError } from "@/shared/api/client";
import { Button } from "@/shared/ui-primitives/button";
import { loadLanguagePreference } from "@/shared/i18n";
import { useApplyUiScale, useUiScalePreference } from "@/shared/hooks/useUiScalePreference";
import { useDocumentTitle } from "@/shared/hooks/useDocumentTitle";
import { ErrorBoundary } from "@/shared/ui-shared/ErrorBoundary";
import { useBranding } from "@/app/BrandingContext";
import { sectionLabelKeyForPath } from "./routeTitle";
import { AssistantProvider, AssistantWindow } from "@/features/assistant";
import { Sheet, SheetContent, SheetTitle } from "@/shared/ui-primitives/sheet";
import ConnectionsWorkspaceOutlet from "@/features/connections/ConnectionsWorkspaceOutlet";
import { PageChromeProvider } from "@/shared/ui-shared/pageChromeContext";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";
import PageState from "@/shared/ui-shared/PageState";

function ShellPageError({ reset }: { reset: () => void }) {
  const { t } = useTranslation();
  return <PageLayout>
    <PageChrome title={t("dataTable.errorTitle")} icon={AlertCircle} />
    <PageState kind="error" title={t("unknownError")} action={<Button onClick={reset}>{t("retry")}</Button>} />
  </PageLayout>;
}

const AppShell = () => {
  const location = useLocation();
  const { t } = useTranslation();
  const { appName } = useBranding();

  // `<instance> · <active section>`, using the same localized nav labels the
  // sidebar shows. Routes outside the shell nav fall back to the bare instance
  // name. This (with the detached-session effect) is the sole writer of
  // `document.title`; branding no longer sets it directly.
  const sectionKey = sectionLabelKeyForPath(location.pathname);
  useDocumentTitle([appName, sectionKey ? t(sectionKey) : null]);

  const langLoadedRef = useRef(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const toggleSidebar = useCallback(() => {
    setSidebarCollapsed((collapsed) => !collapsed);
  }, []);
  const isMobile = useIsMobile();
  const [mobileSidebarOpen, setMobileSidebarOpen] = useState(false);

  const authQuery = useQuery({
    queryKey: ["auth", "me"],
    queryFn: me,
    retry: false,
    refetchOnWindowFocus: false,
  });
  const uiScalePreference = useUiScalePreference(Boolean(authQuery.data));
  useApplyUiScale(uiScalePreference.value);

  // Load user's language preference from the API once after successful auth.
  useEffect(() => {
    if (authQuery.isSuccess && authQuery.data && !langLoadedRef.current) {
      langLoadedRef.current = true;
      void loadLanguagePreference();
    }
  }, [authQuery.isSuccess, authQuery.data]);

  if (authQuery.isPending) {
    return (
      <div className="flex min-h-[var(--app-vh)] items-center justify-center">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (!authQuery.data) {
    if (authQuery.isError && !(authQuery.error instanceof ApiError && authQuery.error.status === 401)) {
      return (
        <main className="flex min-h-[var(--app-vh)] items-center justify-center p-4">
          <div role="alert" className="flex max-w-sm flex-col items-center gap-4 text-center">
            <AlertCircle className="size-8 text-destructive" aria-hidden="true" />
            <h1 className="text-lg font-semibold">{t("dataTable.errorTitle")}</h1>
            <Button onClick={() => { void authQuery.refetch(); }} disabled={authQuery.isFetching}>{t("retry")}</Button>
          </div>
        </main>
      );
    }
    // Confirmed unauthenticated state is redirected by SessionRecovery.
    return null;
  }

  return (
    <PermissionsProvider user={authQuery.data}>
      <AssistantProvider>
        <PageChromeProvider navigationToken={location.key}>
        <div className="relative h-[var(--app-vh)] w-full overflow-hidden">
          <a href="#main-content" className="sr-only z-[100] rounded-md bg-primary p-3 text-primary-foreground focus:not-sr-only focus:absolute focus:left-4 focus:top-4"
            onClick={(event) => { event.preventDefault(); document.getElementById("main-content")?.focus(); }}>
            {t("topBar.skipToContent")}
          </a>
          <div className="relative z-10 flex h-[var(--app-vh)] w-full overflow-hidden">
            {!isMobile && (
              <AppSidebar collapsed={sidebarCollapsed} onToggle={toggleSidebar} />
            )}

            {isMobile && (
              <Sheet open={mobileSidebarOpen} onOpenChange={setMobileSidebarOpen}>
                <SheetContent side="left" hideClose className="w-fit max-w-full p-0" aria-describedby={undefined}
                  onCloseAutoFocus={() => document.getElementById("mobile-navigation-toggle")?.focus()}>
                  <SheetTitle className="sr-only">{t("nav.mainNavigation")}</SheetTitle>
                  <AppSidebar collapsed={false} onToggle={() => setMobileSidebarOpen(false)} onNavigate={() => setMobileSidebarOpen(false)} />
                </SheetContent>
              </Sheet>
            )}

            <div className="flex flex-1 flex-col min-w-0 min-h-0">
              <TopBar
                onToggleMobileSidebar={() => setMobileSidebarOpen(!mobileSidebarOpen)}
                showMobileToggle={isMobile}
              />
              <main id="main-content" tabIndex={-1} className="relative min-h-0 min-w-0 flex-1 overflow-y-auto overflow-x-hidden stable-scrollbar-gutter outline-none">
                <ConnectionsWorkspaceOutlet />
                {/*
                  The page gets its own boundary INSIDE the shell. The only
                  boundary used to sit above <Routes>, so a render error in one
                  screen replaced the whole application — sidebar, top bar and
                  status bar included — leaving no way to navigate anywhere
                  else. Scoped here, a crashed page keeps the frame around it,
                  and the route key clears a caught error before the next
                  page registers chrome, without replacing the shell/workspace.
                */}
                <ErrorBoundary key={location.pathname} scope="Page" fallback={({ reset }) => <ShellPageError reset={reset} />}>
                   <Suspense fallback={<PageLayout><PageState kind="loading" /></PageLayout>}>
                    <Outlet />
                  </Suspense>
                </ErrorBoundary>
              </main>
              <StatusBar />
            </div>
          </div>
          <AssistantWindow />
        </div>
        </PageChromeProvider>
      </AssistantProvider>
    </PermissionsProvider>
  );
};

export default AppShell;
