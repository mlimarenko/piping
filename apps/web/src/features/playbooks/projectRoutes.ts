import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

/** Relative href to a project's file editor. */
export function projectFilesHref(id: string): string {
  return projectSectionHref(id, "files");
}

export function projectSectionHref(id: string, section: "files" | "jobs" | "source" | "settings"): string {
  return `${PLAYBOOK_TAB_ROUTES.projects}/${encodeURIComponent(id)}/${section}`;
}
