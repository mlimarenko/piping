export type PlaybooksTab =
  | "projects"
  | "job-templates"
  | "settings"
  | "execution-environments"
  | "target-sets"
  | "assignments"
  | "schedules"
  | "runs";

export const PLAYBOOK_TAB_ROUTES: Record<PlaybooksTab, string> = {
  projects: "/playbooks/projects",
  "job-templates": "/playbooks/jobs",
  settings: "/playbooks/settings",
  "execution-environments": "/playbooks/settings/execution-environments",
  "target-sets": "/playbooks/settings/target-sets",
  assignments: "/playbooks/settings/assignments",
  schedules: "/playbooks/schedules",
  runs: "/playbooks/runs",
};

/** The settings surfaces below the Settings tab. */
export const PLAYBOOK_SETTINGS_TABS = [
  "execution-environments",
  "target-sets",
  "assignments",
] as const;
export type PlaybookSettingsTab = (typeof PLAYBOOK_SETTINGS_TABS)[number];

/** The Settings tab and its surfaces form one settings section in the top row. */
export function isPlaybookSettingsTab(
  tab: PlaybooksTab,
): tab is PlaybookSettingsTab | "settings" {
  return (
    tab === "settings" ||
    (PLAYBOOK_SETTINGS_TABS as readonly PlaybooksTab[]).includes(tab)
  );
}

export function playbooksTabFromPath(
  pathname: string,
  fallback: PlaybooksTab = "projects",
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
 *   /playbooks/runs?group=<runGroupId>&name=<playbookName>
 *
 * Opening this URL selects the Runs tab and opens that group's RunDetailPanel
 * `name` is carried in the URL — not re-resolved on the target page — because the caller
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

/**
 * Sole builder/parser for the target-set preview deep link:
 *
 *   /playbooks/settings/target-sets?targetSet=<id>
 *
 * The launch dialog links here so the operator can review the full resolved
 * composition the run will use.
 */
export const PLAYBOOK_TARGET_SET_PARAM = "targetSet";

export function playbookTargetSetPreviewHref(targetSetId: string): string {
  const params = new URLSearchParams({ [PLAYBOOK_TARGET_SET_PARAM]: targetSetId });
  return `${PLAYBOOK_TAB_ROUTES["target-sets"]}?${params.toString()}`;
}
