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

/** Playbook editor (catalog list at `/playbooks`). */
export const PLAYBOOK_EDITOR_NEW_PATH = "/playbooks/new";
export const PLAYBOOK_EDITOR_EDIT_PATH = "/playbooks/:id/edit";

/** Copy-draft query param carried on the create route ("use as template"). */
export const PLAYBOOK_EDITOR_SOURCE_PARAM = "source";
/** Editor tab query param (`tab=history` opens version history). */
export const PLAYBOOK_EDITOR_TAB_PARAM = "tab";

export function playbookEditorNewHref(): string {
  return PLAYBOOK_EDITOR_NEW_PATH;
}

export function playbookEditorEditHref(id: string): string {
  return `/playbooks/${encodeURIComponent(id)}/edit`;
}

/** Copy-draft ("use as template" / duplicate) for a source playbook. */
export function playbookEditorCopyHref(sourceId: string): string {
  const params = new URLSearchParams({ [PLAYBOOK_EDITOR_SOURCE_PARAM]: sourceId });
  return `${PLAYBOOK_EDITOR_NEW_PATH}?${params.toString()}`;
}

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

export const SERVICE_PRINCIPAL_EDITOR_NEW_PATH = `${PLAYBOOK_TAB_ROUTES["service-principals"]}/new`;

export function servicePrincipalEditorNewHref(): string {
  return SERVICE_PRINCIPAL_EDITOR_NEW_PATH;
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

export function jobTemplateEditorNewHref(): string {
  return JOB_TEMPLATE_EDITOR_NEW_PATH;
}

/** Return-target query param on the target-set create route (`returnTo=assignment`). */
export const TARGET_SET_RETURN_TO_PARAM = "returnTo";
/** Seed query params on the job-template create route. */
export const JOB_TEMPLATE_EXECUTION_ENVIRONMENT_PARAM = "executionEnvironment";
export const JOB_TEMPLATE_PROJECT_PARAM = "project";

/** Target-set create opened from the assignment dialog: return there on save/cancel. */
export function targetSetEditorNewForAssignmentHref(): string {
  return `${TARGET_SET_EDITOR_NEW_PATH}?${TARGET_SET_RETURN_TO_PARAM}=assignment`;
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
