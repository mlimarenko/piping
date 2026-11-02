import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

/** Relative href to a project's file editor. */
export function projectFilesHref(id: string): string {
  return `${PLAYBOOK_TAB_ROUTES.projects}/${encodeURIComponent(id)}/files`;
}
