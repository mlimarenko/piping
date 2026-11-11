import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

/**
 * Canonical automation editor routes (S3-e).
 *
 * One path per editor (create + edit where the entity is editable), all following
 * the same `<list>/new` and `<list>/:id/edit` convention as `ProjectFilesPage`
 * and the inventory/observability editors. App.tsx mounts these exact paths and
 * PlaybooksPage navigates to them via the href builders below, so the route
 * contract lives in exactly one place.
 */

export const TARGET_SET_EDITOR_NEW_PATH = `${PLAYBOOK_TAB_ROUTES["target-sets"]}/new`;
export const TARGET_SET_EDITOR_EDIT_PATH = `${PLAYBOOK_TAB_ROUTES["target-sets"]}/:id/edit`;

export function targetSetEditorNewHref(): string {
  return TARGET_SET_EDITOR_NEW_PATH;
}

export function targetSetEditorEditHref(id: string): string {
  return `${PLAYBOOK_TAB_ROUTES["target-sets"]}/${encodeURIComponent(id)}/edit`;
}

export const SCHEDULE_EDITOR_NEW_PATH = `${PLAYBOOK_TAB_ROUTES.schedules}/new`;
export const SCHEDULE_EDITOR_EDIT_PATH = `${PLAYBOOK_TAB_ROUTES.schedules}/:id/edit`;

export function scheduleEditorNewHref(): string {
  return SCHEDULE_EDITOR_NEW_PATH;
}

export function scheduleEditorEditHref(id: string): string {
  return `${PLAYBOOK_TAB_ROUTES.schedules}/${encodeURIComponent(id)}/edit`;
}

export const EXECUTION_ENVIRONMENT_EDITOR_NEW_PATH = `${PLAYBOOK_TAB_ROUTES["execution-environments"]}/new`;

export function executionEnvironmentEditorNewHref(): string {
  return EXECUTION_ENVIRONMENT_EDITOR_NEW_PATH;
}

export const PROJECT_EDITOR_NEW_PATH = `${PLAYBOOK_TAB_ROUTES.projects}/new`;

export function projectEditorNewHref(): string {
  return PROJECT_EDITOR_NEW_PATH;
}

export const JOB_TEMPLATE_EDITOR_NEW_PATH = `${PLAYBOOK_TAB_ROUTES["job-templates"]}/new`;
export const JOB_TEMPLATE_EDITOR_EDIT_PATH = `${PLAYBOOK_TAB_ROUTES["job-templates"]}/:jobId/edit`;

export function jobTemplateEditorEditHref(id: string): string {
  return `${PLAYBOOK_TAB_ROUTES["job-templates"]}/${encodeURIComponent(id)}/edit`;
}

export function jobTemplateEditorNewHref(): string {
  return JOB_TEMPLATE_EDITOR_NEW_PATH;
}

/** The target set whose draft is copied into the create editor. */
export const TARGET_SET_DUPLICATE_PARAM = "duplicate";
/** The empty-state starter template seeded into the create editor. */
export const TARGET_SET_TEMPLATE_PARAM = "template";
/** Seed query params on the job-template create route. */
export const JOB_TEMPLATE_EXECUTION_ENVIRONMENT_PARAM = "executionEnvironment";
export const JOB_TEMPLATE_PROJECT_PARAM = "project";

/** Target-set create opened as a copy of an existing set: the editor pre-fills. */
export function targetSetEditorNewForDuplicateHref(id: string): string {
  const params = new URLSearchParams({ [TARGET_SET_DUPLICATE_PARAM]: id });
  return `${TARGET_SET_EDITOR_NEW_PATH}?${params.toString()}`;
}

/** Target-set create opened from an empty-state starter template. */
export function targetSetEditorNewForTemplateHref(template: string): string {
  const params = new URLSearchParams({ [TARGET_SET_TEMPLATE_PARAM]: template });
  return `${TARGET_SET_EDITOR_NEW_PATH}?${params.toString()}`;
}

/**
 * Only automation pages and the inventory source list may receive a return
 * flow. URL normalization rejects traversal out of those routes.
 */
export function isTargetSetReturnHref(href: string): boolean {
  if (!href.startsWith("/") || href.startsWith("//") || href.includes("\\") || href.trim() !== href) return false;
  try {
    const url = new URL(href, "https://return.invalid");
    return url.origin === "https://return.invalid" && (url.pathname === "/playbooks" ||
      url.pathname.startsWith("/playbooks/") || url.pathname === "/inventory/resources");
  } catch { return false; }
}

/** Job-template create seeded with an execution environment. */
export function jobTemplateEditorNewForExecutionEnvironmentHref(environmentId: string): string {
  const params = new URLSearchParams({ [JOB_TEMPLATE_EXECUTION_ENVIRONMENT_PARAM]: environmentId });
  return `${JOB_TEMPLATE_EDITOR_NEW_PATH}?${params.toString()}`;
}

/** Job-template create seeded with a project source. */
export function jobTemplateEditorNewForProjectHref(projectId: string): string {
  const params = new URLSearchParams({ [JOB_TEMPLATE_PROJECT_PARAM]: projectId });
  return `${JOB_TEMPLATE_EDITOR_NEW_PATH}?${params.toString()}`;
}
