import type { AutomationDocSlug } from "@/shared/lib/automationDocs";
import type { PlaybooksTab } from "./playbookNavigation";

/** The documentation page that describes each automation tab. */
export const PLAYBOOK_TAB_DOCS: Record<PlaybooksTab, { slug: AutomationDocSlug; hash?: string }> = {
  projects: { slug: "repositories" },
  "job-templates": { slug: "jobs-and-runs" },
  runs: { slug: "jobs-and-runs", hash: "results" },
  schedules: { slug: "schedules" },
  "execution-environments": { slug: "jobs-and-runs", hash: "execution-environments" },
  "target-sets": { slug: "targets" },
  assignments: { slug: "targets", hash: "assignments" },
  settings: { slug: "automation" },
};
