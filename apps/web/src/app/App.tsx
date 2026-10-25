import { lazy, Suspense } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Route, Routes, Navigate } from "react-router";
import { Loader2 } from "lucide-react";
import { Toaster as Sonner } from "@/shared/ui-primitives/sonner";
import { Toaster } from "@/shared/ui-primitives/toaster";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import { ThemeProvider } from "@/shared/hooks/use-theme";
import { ErrorBoundary } from "@/shared/ui-shared/ErrorBoundary";
import { captureException } from "@/shared/lib/observability";
import { BrandingProvider } from "@/app/BrandingContext";
import DesktopShellFrame from "@/app/desktop/DesktopShellFrame";

import AppShell from "@/app/layout/AppShell";
import PageLayout from "@/shared/ui-shared/PageLayout";
import { Outlet } from "react-router";
import AuthLogin from "@/features/auth/AuthLogin";
import AuthRedirect from "@/features/auth/AuthRedirect";
import SessionRecovery from "@/features/auth/SessionRecovery";
import { RequireCapability } from "@/shared/auth/permissions";
import {
  PLAYBOOK_EDITOR_NEW_PATH,
  PLAYBOOK_EDITOR_EDIT_PATH,
  TARGET_SET_EDITOR_NEW_PATH,
  TARGET_SET_EDITOR_EDIT_PATH,
  SCHEDULE_EDITOR_NEW_PATH,
  SCHEDULE_EDITOR_EDIT_PATH,
  SERVICE_PRINCIPAL_EDITOR_NEW_PATH,
  EXECUTION_ENVIRONMENT_EDITOR_NEW_PATH,
  PROJECT_EDITOR_NEW_PATH,
  JOB_TEMPLATE_EDITOR_NEW_PATH,
} from "@/features/playbooks/playbookEditorRoutes";
import { PLAYBOOK_DETAIL_PATH } from "@/features/playbooks/playbookDetailRoutes";

// Hot-path pages: rendered on the default route or on every navigation.
// NotFound is tiny and kept eager. ConnectionsWorkspace is lazy because the
// default route always redirects through auth first — it is never rendered
// before the shell hydrates.
import NotFound from "@/pages/NotFound";

// Heavy / off-the-default-route pages: split out so the initial main chunk
// stays small. Each lazy() yields a separate vite chunk.
const ConnectionConfig = lazy(() => import("@/features/connections/connection-config/ConnectionConfig"));
const ConnectionConfigEditor = lazy(() => import("@/features/connections/connection-config/ConnectionConfigEditor"));
const ConnectionConfigDetail = lazy(() => import("@/features/connections/connection-config/ConnectionConfigDetail"));
const CustomFields = lazy(() => import("@/features/inventory/CustomFields"));
const ActiveConnections = lazy(() => import("@/features/connections/ActiveConnections"));
const ProxyNodes = lazy(() => import("@/features/connections/ProxyNodes"));
const DeviceDashboardPage = lazy(() => import("@/features/device-dashboard/DeviceDashboardPage"));
const SyncPage = lazy(() => import("@/features/inventory/SyncPage"));
const SyncMappingList = lazy(() => import("@/features/inventory/SyncMappingList"));
const SyncMappingEditor = lazy(() => import("@/features/inventory/SyncMappingEditor"));
const InventoryResources = lazy(() => import("@/features/inventory/InventoryResources"));
const DataflowsPage = lazy(() => import("@/features/inventory/DataflowsPage"));
const DataflowEditorPage = lazy(() => import("@/features/inventory/DataflowEditorPage"));
const EntityResourceEditorPage = lazy(() => import("@/features/inventory/EntityResourceEditorPage"));
const InventoryResourceDetailPage = lazy(() => import("@/features/inventory/InventoryResourceDetailPage"));
const InventoryTemplates = lazy(() => import("@/features/inventory/InventoryTemplates"));
const TemplateEditorPage = lazy(() => import("@/features/inventory/TemplateEditorPage"));
const BackupPage = lazy(() => import("@/features/backup/BackupPage"));
const ObservabilityPage = lazy(() => import("@/features/observability/ObservabilityPage"));
const PlaybooksPage = lazy(() => import("@/features/playbooks/PlaybooksPage"));
const ProjectFilesPage = lazy(() => import("@/features/playbooks/ProjectFilesPage"));
const PlaybookEditorPage = lazy(() => import("@/features/playbooks/PlaybookEditorPage"));
const PlaybookDetailPage = lazy(() => import("@/features/playbooks/PlaybookDetailPage"));
const TargetSetEditorPage = lazy(() => import("@/features/playbooks/TargetSetEditorPage"));
const ScheduleEditorPage = lazy(() => import("@/features/playbooks/ScheduleEditorPage"));
const ServicePrincipalEditorPage = lazy(() => import("@/features/playbooks/ServicePrincipalEditorPage"));
const ExecutionEnvironmentEditorPage = lazy(() => import("@/features/playbooks/ExecutionEnvironmentEditorPage"));
const ProjectEditorPage = lazy(() => import("@/features/playbooks/ProjectEditorPage"));
const JobTemplateEditorPage = lazy(() => import("@/features/playbooks/JobTemplateEditorPage"));
const AutomationSecretsPage = lazy(() => import("@/features/automation-secrets/AutomationSecretsPage"));
const McpAdminPage = lazy(() => import("@/features/mcp/McpAdminPage"));
const AccessControlWorkspace = lazy(() => import("@/features/access-control/AccessControlWorkspace"));
const AuditLog = lazy(() => import("@/features/audit/AuditLog"));
const LicensePage = lazy(() => import("@/features/licensing/LicensePage"));
const WebhooksPage = lazy(() => import("@/features/webhooks/WebhooksPage"));
const AppearancePage = lazy(() => import("@/features/branding/AppearancePage"));
const AdminSettingsPage = lazy(() => import("@/features/settings/AdminSettingsPage"));
const UserSettings = lazy(() => import("@/features/user-settings/UserSettings"));
const ApiDocsPage = lazy(() => import("@/features/api-docs/ApiDocsPage"));
// DetachedSession pulls in noVNC/guacamole/xterm bundles (~600KB) for the
// session viewer; never load it on the default route.
const DetachedSession = lazy(() => import("@/pages/DetachedSession"));
const DetachedGroup = lazy(() => import("@/pages/DetachedGroup"));
const ProxyNodeEditor = lazy(() => import("@/features/connections/ProxyNodeEditor"));
const ProxyNodeDetail = lazy(() => import("@/features/connections/ProxyNodeDetail"));
const ProxyAssignmentEditor = lazy(() => import("@/features/connections/ProxyAssignmentEditor"));
const QuerySourceEditor = lazy(() => import("@/features/observability/QuerySourceEditor"));
const ExporterAssignmentEditor = lazy(() => import("@/features/observability/ExporterAssignmentEditor"));
const ExporterAssignmentDetail = lazy(() => import("@/features/observability/ExporterAssignmentDetail"));
const QuerySourceDetail = lazy(() => import("@/features/observability/QuerySourceDetail"));
const MetricQueryEditor = lazy(() => import("@/features/observability/MetricQueryEditor"));
const MetricQueryDetail = lazy(() => import("@/features/observability/MetricQueryDetail"));
const MetricSetEditor = lazy(() => import("@/features/observability/MetricSetEditor"));
const MetricSetDetail = lazy(() => import("@/features/observability/MetricSetDetail"));
const AlertSourceEditor = lazy(() => import("@/features/observability/AlertSourceEditor"));
const AlertSourceDetail = lazy(() => import("@/features/observability/AlertSourceDetail"));
const UserCredentialEditor = lazy(() => import("@/features/user-settings/UserCredentialEditor"));
const UserTokenEditor = lazy(() => import("@/features/user-settings/UserTokenEditor"));

function captureMutationError(error: Error): void {
  const tags: Record<string, string> = { tanstackQuery: "mutation" };
  const traceId = (error as { traceId?: unknown }).traceId;
  if (typeof traceId === "string") tags.traceId = traceId;
  captureException(error, { tags });
}

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => {
        const status = (error as { status?: number })?.status;
        if (status === 401 || status === 403 || status === 404) return false;
        return failureCount < 2;
      },
      refetchOnWindowFocus: false,
      staleTime: 30_000,
    },
    mutations: {
      onError: captureMutationError,
    },
  },
});

const RouteFallback = () => (
  <div className="flex min-h-[40vh] items-center justify-center">
    <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
  </div>
);

// Router base path. In the normal app `import.meta.env.BASE_URL` is "/", so the
// basename is undefined (default). The demo build (`--mode demo`) sets the Vite
// base to "/demo-app/", so the router is mounted under "/demo-app" to match the
// portal's embed path — additive, no effect on the production app.
const ROUTER_BASENAME =
  import.meta.env.BASE_URL && import.meta.env.BASE_URL !== "/"
    ? import.meta.env.BASE_URL.replace(/\/$/, "")
    : undefined;

const App = () => (
  <QueryClientProvider client={queryClient}>
    <ThemeProvider>
      <BrandingProvider>
        <TooltipProvider>
          <Toaster />
          <Sonner />
          <DesktopShellFrame>
            <BrowserRouter basename={ROUTER_BASENAME}>
              <SessionRecovery />
              <ErrorBoundary scope="App">
                <Suspense fallback={<RouteFallback />}>
                  <Routes>
                    <Route path="/auth/login" element={<AuthLogin />} />
                    <Route path="/auth/redirect" element={<AuthRedirect />} />
                    <Route path="/connections/detached/:connectionId" element={<DetachedSession />} />
                    <Route path="/connections/detached-group" element={<DetachedGroup />} />

                    <Route element={<AppShell />}>
                      <Route path="/" element={<Navigate to="/connections" replace />} />
                      {/* Connections/inventory route gates mirror the sidebar
                          `gate:` entries in app/navigation.ts — a deep link must not
                          render a privileged frame the nav already hides. */}
                      <Route path="/connections" element={null} />
                      {/* All management routes share one canvas, in every load and permission state. */}
                      <Route element={<PageLayout fill><Outlet /></PageLayout>}>
                        <Route path="/connections/config" element={<RequireCapability check={(p) => p.can("READ", "CONFIG_CONTEXT")}><ConnectionConfig /></RequireCapability>} />
                        <Route path="/automation/runs" element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_RUN_GROUP")}><PlaybooksPage initialTab="runs" /></RequireCapability>} />
                        <Route path="/connections/config/:id" element={<RequireCapability check={(p) => p.can("READ", "CONFIG_CONTEXT")}><ConnectionConfigDetail /></RequireCapability>} />
                      <Route path="/connections/config/new" element={<RequireCapability check={(p) => p.can("READ", "CONFIG_CONTEXT")}><ConnectionConfigEditor /></RequireCapability>} />
                      <Route path="/connections/config/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "CONFIG_CONTEXT")}><ConnectionConfigEditor /></RequireCapability>} />

                      <Route path="/connections/active" element={<RequireCapability check={(p) => p.can("READ", "CONNECTION_SESSION")}><ActiveConnections /></RequireCapability>} />
                      <Route path="/connections/proxies" element={<RequireCapability check={(p) => p.can("READ", "PROXY_NODE")}><ProxyNodes /></RequireCapability>} />
                      <Route path="/inventory/sync" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><SyncPage /></RequireCapability>} />
                      <Route path="/inventory/sync/mappings" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><SyncMappingList /></RequireCapability>} />
                      <Route path="/inventory/sync/:providerId/mappings" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><SyncMappingList /></RequireCapability>} />
                      <Route path="/inventory/dataflows" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><DataflowsPage /></RequireCapability>} />
                      <Route path="/inventory/templates" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><InventoryTemplates /></RequireCapability>} />
                      <Route path="/api-docs" element={<ApiDocsPage />} />
                      <Route path="/playbooks" element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK")}><PlaybooksPage initialTab="catalog" /></RequireCapability>} />
                      <Route
                        path="/automation/projects"
                        element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_PROJECT")}><PlaybooksPage initialTab="projects" /></RequireCapability>}
                      />
                      <Route
                        path="/automation/job-templates"
                        element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_JOB_TEMPLATE")}><PlaybooksPage initialTab="job-templates" /></RequireCapability>}
                      />
                      <Route
                        path="/automation/execution-environments"
                        element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_EXECUTION_ENVIRONMENT")}><PlaybooksPage initialTab="execution-environments" /></RequireCapability>}
                      />
                      <Route
                        path="/automation/assignments"
                        element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_ASSIGNMENT")}><PlaybooksPage initialTab="assignments" /></RequireCapability>}
                      />
                      <Route
                        path="/automation/target-sets"
                        element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_TARGET_SET")}><PlaybooksPage initialTab="target-sets" /></RequireCapability>}
                      />
                      <Route
                        path="/automation/schedules"
                        element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_SCHEDULE")}><PlaybooksPage initialTab="schedules" /></RequireCapability>}
                      />
                      <Route
                        path="/automation/service-principals"
                        element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_SERVICE_PRINCIPAL")}><PlaybooksPage initialTab="service-principals" /></RequireCapability>}
                      />
                      <Route path="/automation/secrets" element={<RequireCapability check={(p) => p.can("READ", "SECRET_VARIABLE")}><AutomationSecretsPage /></RequireCapability>} />


                      <Route path="/access-control" element={<RequireCapability check={(p) => p.can("READ", "USER") || p.can("READ", "ROLE") || p.can("READ", "POLICY") || p.can("READ", "GROUP") || p.can("READ", "KEYCLOAK")}><AccessControlWorkspace /></RequireCapability>} />
                      <Route path="/user/settings" element={<UserSettings />} />
                      <Route path="/observability" element={<ObservabilityPage />} />
                      <Route path="/inventory/custom-fields" element={<CustomFields />} />
                      <Route path="/inventory/resources" element={<InventoryResources />} />

                        <Route path="/inventory/resources/:entity/:id" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><InventoryResourceDetailPage /></RequireCapability>} />
                        <Route path={PLAYBOOK_DETAIL_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK")}><PlaybookDetailPage /></RequireCapability>} />
                        <Route path="/connections/proxies/:id" element={<RequireCapability check={(p) => p.can("READ", "PROXY_NODE")}><ProxyNodeDetail /></RequireCapability>} />
                        <Route path="/observability/sources/:id" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><QuerySourceDetail /></RequireCapability>} />
                        <Route path="/observability/exporter-assignments/:id" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><ExporterAssignmentDetail /></RequireCapability>} />
                        <Route path="/observability/metrics/:id" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><MetricQueryDetail /></RequireCapability>} />
                        <Route path="/observability/sets/:id" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><MetricSetDetail /></RequireCapability>} />
                        <Route path="/observability/alert-sources/:id" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><AlertSourceDetail /></RequireCapability>} />
                        <Route path={PLAYBOOK_EDITOR_NEW_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK")}><PlaybookEditorPage /></RequireCapability>} />
                        <Route path={PLAYBOOK_EDITOR_EDIT_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK")}><PlaybookEditorPage /></RequireCapability>} />
                        <Route path={TARGET_SET_EDITOR_NEW_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_TARGET_SET")}><TargetSetEditorPage /></RequireCapability>} />
                        <Route path={TARGET_SET_EDITOR_EDIT_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_TARGET_SET")}><TargetSetEditorPage /></RequireCapability>} />
                        <Route path={SCHEDULE_EDITOR_NEW_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_SCHEDULE")}><ScheduleEditorPage /></RequireCapability>} />
                        <Route path={SCHEDULE_EDITOR_EDIT_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_SCHEDULE")}><ScheduleEditorPage /></RequireCapability>} />
                        <Route path="/automation/projects/:id/files" element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_PROJECT")}><ProjectFilesPage /></RequireCapability>} />
                        <Route path="/inventory/resources/:entity/new" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><EntityResourceEditorPage /></RequireCapability>} />
                        <Route path="/inventory/resources/:entity/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><EntityResourceEditorPage /></RequireCapability>} />
                        <Route path="/inventory/templates/new" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><TemplateEditorPage /></RequireCapability>} />
                        <Route path="/inventory/templates/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><TemplateEditorPage /></RequireCapability>} />
                        <Route path="/inventory/dataflows/new" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><DataflowEditorPage /></RequireCapability>} />
                        <Route path="/inventory/dataflows/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><DataflowEditorPage /></RequireCapability>} />
                        <Route path="/inventory/sync/mappings/:id/:mode" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><SyncMappingEditor /></RequireCapability>} />
                        <Route path="/inventory/sync/mappings/new" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><SyncMappingEditor /></RequireCapability>} />
                        <Route path="/inventory/sync/:providerId/mappings/new" element={<RequireCapability check={(p) => p.can("READ", "INVENTORY")}><SyncMappingEditor /></RequireCapability>} />
                        <Route path="/connections/proxies/assignments/new" element={<RequireCapability check={(p) => p.can("READ", "PROXY_NODE")}><ProxyAssignmentEditor /></RequireCapability>} />
                        <Route path="/connections/proxies/assignments/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "PROXY_NODE")}><ProxyAssignmentEditor /></RequireCapability>} />
                        <Route path="/connections/proxies/new" element={<RequireCapability check={(p) => p.can("READ", "PROXY_NODE")}><ProxyNodeEditor /></RequireCapability>} />
                        <Route path="/connections/proxies/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "PROXY_NODE")}><ProxyNodeEditor /></RequireCapability>} />
                        <Route path="/observability/sources/new" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><QuerySourceEditor /></RequireCapability>} />
                        <Route path="/observability/exporter-assignments/new" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><ExporterAssignmentEditor /></RequireCapability>} />
                        <Route path="/observability/exporter-assignments/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><ExporterAssignmentEditor /></RequireCapability>} />
                        <Route path="/observability/sources/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><QuerySourceEditor /></RequireCapability>} />
                        <Route path="/observability/metrics/new" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><MetricQueryEditor /></RequireCapability>} />
                        <Route path="/observability/metrics/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><MetricQueryEditor /></RequireCapability>} />
                        <Route path="/observability/sets/new" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><MetricSetEditor /></RequireCapability>} />
                        <Route path="/observability/sets/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><MetricSetEditor /></RequireCapability>} />
                        <Route path="/observability/alert-sources/new" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><AlertSourceEditor /></RequireCapability>} />
                        <Route path="/observability/alert-sources/:id/edit" element={<RequireCapability check={(p) => p.can("READ", "MONITORING")}><AlertSourceEditor /></RequireCapability>} />
                        <Route path={SERVICE_PRINCIPAL_EDITOR_NEW_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_SERVICE_PRINCIPAL")}><ServicePrincipalEditorPage /></RequireCapability>} />
                        <Route path={EXECUTION_ENVIRONMENT_EDITOR_NEW_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_EXECUTION_ENVIRONMENT")}><ExecutionEnvironmentEditorPage /></RequireCapability>} />
                        <Route path={PROJECT_EDITOR_NEW_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_PROJECT")}><ProjectEditorPage /></RequireCapability>} />
                        <Route path={JOB_TEMPLATE_EDITOR_NEW_PATH} element={<RequireCapability check={(p) => p.can("READ", "PLAYBOOK_JOB_TEMPLATE")}><JobTemplateEditorPage /></RequireCapability>} />
                        <Route path="/user/settings/credentials/new" element={<UserCredentialEditor />} />
                        <Route path="/user/settings/credentials/:id/edit" element={<UserCredentialEditor />} />
                        <Route path="/user/settings/tokens/new" element={<UserTokenEditor />} />
                        <Route path="/connections/devices/:targetId" element={<RequireCapability check={(p) => p.can("READ", "CONNECTION_TARGET")}><DeviceDashboardPage /></RequireCapability>} />
                        <Route path="/mcp" element={<RequireCapability check={(p) => p.isAdmin}><McpAdminPage /></RequireCapability>} />
                        <Route path="/audit-logs" element={<RequireCapability check={(p) => p.can("VIEW_AUDIT", "AUDIT_EVENT")}><AuditLog /></RequireCapability>} />
                        <Route path="/administration/license" element={<RequireCapability check={(p) => p.can("READ", "LICENSE")}><LicensePage /></RequireCapability>} />
                        <Route path="/administration/webhooks" element={<RequireCapability check={(p) => p.can("READ", "WEBHOOK")}><WebhooksPage /></RequireCapability>} />
                        <Route path="/administration/branding" element={<RequireCapability check={(p) => p.canAny("BRANDING", ["READ", "UPDATE"])}><AppearancePage /></RequireCapability>} />
                        <Route path="/administration/settings" element={<RequireCapability check={(p) => p.can("READ", "SETTINGS")}><AdminSettingsPage /></RequireCapability>} />
                        <Route path="/administration/backup" element={<RequireCapability check={(p) => p.can("READ", "BACKUP")}><BackupPage /></RequireCapability>} />
                      </Route>
                    </Route>

                    <Route path="*" element={<NotFound />} />
                  </Routes>
                </Suspense>
              </ErrorBoundary>
            </BrowserRouter>
          </DesktopShellFrame>
        </TooltipProvider>
      </BrandingProvider>
    </ThemeProvider>
  </QueryClientProvider>
);

export default App;
