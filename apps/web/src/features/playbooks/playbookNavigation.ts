export type PlaybooksTab =
  | "catalog"
  | "projects"
  | "job-templates"
  | "execution-environments"
  | "assignments"
  | "target-sets"
  | "schedules"
  | "service-principals"
  | "runs";

export const PLAYBOOK_TAB_ROUTES: Record<PlaybooksTab, string> = {
  catalog: "/playbooks",
  projects: "/automation/projects",
  "job-templates": "/automation/job-templates",
  "execution-environments": "/automation/execution-environments",
  assignments: "/automation/assignments",
  "target-sets": "/automation/target-sets",
  schedules: "/automation/schedules",
  "service-principals": "/automation/service-principals",
  runs: "/automation/runs",
};

export function playbooksTabFromPath(
  pathname: string,
  fallback: PlaybooksTab = "catalog",
): PlaybooksTab {
  for (const [tab, path] of Object.entries(PLAYBOOK_TAB_ROUTES) as Array<
    [PlaybooksTab, string]
  >) {
    if (pathname === path) return tab;
  }
  return fallback;
}

/**
 * Sole builder/parser for the Runs-tab deep-link contract:
 *
 *   /automation/runs?group=<runGroupId>&name=<playbookName>
 *
 * Opening this URL selects the Runs tab and opens that group's RunDetailPanel
 * (see PlaybooksPage's deep-link effect). `name` is carried in the URL — not
 * re-resolved from the catalog on the target page — because the caller
 * (a Connections playbook tab) already knows it.
 */
export const PLAYBOOK_RUN_GROUP_PARAM = "group";
export const PLAYBOOK_RUN_GROUP_NAME_PARAM = "name";

/** Relative href to the Runs tab, focused on a specific run group. */
export function playbookRunGroupHref(groupId: string, playbookName: string): string {
  const params = new URLSearchParams({
    [PLAYBOOK_RUN_GROUP_PARAM]: groupId,
    [PLAYBOOK_RUN_GROUP_NAME_PARAM]: playbookName,
  });
  return `${PLAYBOOK_TAB_ROUTES.runs}?${params.toString()}`;
}
