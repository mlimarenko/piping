/**
 * Public documentation routes for the automation surfaces.
 *
 * The slugs are the wiki page slugs published under `https://piping.space/docs`.
 * One owner for the URL shape: every section and inline help link resolves
 * through `automationDocUrl`, so a renamed page does not leave a stale
 * hard-coded link in the UI.
 */
export const AUTOMATION_DOCS_BASE = "https://piping.space/docs";

export type AutomationDocSlug =
  | "automation"
  | "repositories"
  | "jobs-and-runs"
  | "targets"
  | "schedules"
  | "playbook-schema"
  | "playbook-runtime"
  | "ansible-cli"
  | "playbook-api"
  | "secrets";

export function automationDocUrl(slug: AutomationDocSlug, hash?: string): string {
  return `${AUTOMATION_DOCS_BASE}/${slug}${hash ? `#${hash}` : ""}`;
}
