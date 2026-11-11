import { describe, expect, it } from "vitest";
import { resolveNavigation, visibleNavigation } from "./navigation";
import type { PermissionsValue } from "@/shared/auth/permissionsContext";
import i18n from "@/shared/i18n";
import { PLAYBOOK_TAB_ROUTES, playbookRunGroupHref, playbooksTabFromPath } from "@/features/playbooks/playbookNavigation";
import { jobTemplateEditorNewHref, scheduleEditorNewHref, targetSetEditorEditHref } from "@/features/playbooks/playbookEditorRoutes";
import { playbookReturnHref } from "@/features/playbooks/playbookDetailRoutes";

function permissions(readable: string[]): PermissionsValue {
  const can = (action: string, resource: string) => action === "READ" && readable.includes(resource);
  return { roles: [], isAdmin: false, can, canAny: (resource, actions) => actions.some(action => can(action, resource)), hasRole: () => false };
}

describe("automation navigation", () => {
  it("resolves canonical list and editor destinations", () => {
    expect(playbooksTabFromPath("/playbooks/jobs")).toBe("job-templates");
    expect(jobTemplateEditorNewHref()).toBe("/playbooks/jobs/new");
    expect(scheduleEditorNewHref()).toBe("/playbooks/schedules/new");
    expect(targetSetEditorEditHref("a b")).toBe("/playbooks/settings/target-sets/a%20b/edit");
    expect(playbookRunGroupHref("group 1", "Job")).toBe("/playbooks/runs?group=group+1&name=Job");
    expect(resolveNavigation("/playbooks/projects/abc/files").section?.labelKey).toBe("nav.automation");
  });

  it("lands on a readable surface and hides the section when none is readable", () => {
    const visible = visibleNavigation(permissions(["PLAYBOOK_SCHEDULE"]));
    expect(visible.find(section => section.labelKey === "nav.automation")?.items[0]?.path).toBe(PLAYBOOK_TAB_ROUTES.schedules);
    expect(visibleNavigation(permissions(["PLAYBOOK"])).find(section => section.labelKey === "nav.automation")).toBeUndefined();
  });

  it("gives each automation area its own item label", () => {
    const section = visibleNavigation(permissions(["SECRET_VARIABLE", "PLAYBOOK_PROJECT", "PLAYBOOK_JOB_TEMPLATE", "PLAYBOOK_RUN_GROUP", "PLAYBOOK_SCHEDULE", "PLAYBOOK_EXECUTION_ENVIRONMENT", "PLAYBOOK_TARGET_SET", "PLAYBOOK_ASSIGNMENT", "PLAYBOOK_SERVICE_PRINCIPAL"]))
      .find(entry => entry.labelKey === "nav.automation");
    expect(section?.items.map(item => item.labelKey)).toEqual(["navItems.automationSecrets", "navItems.playbooks"]);
    expect(section?.items.map(item => item.path)).toEqual(["/automation/secrets", PLAYBOOK_TAB_ROUTES.projects]);
    expect(section?.items.some(item => item.labelKey === section.labelKey)).toBe(false);
    expect(i18n.t("navItems.playbooks")).not.toBe(i18n.t("nav.automation"));
  });

  it("returns only to canonical local automation destinations", () => {    expect(playbookReturnHref("/playbooks/jobs?search=deploy")).toBe("/playbooks/jobs?search=deploy");
    expect(playbookReturnHref("/playbooks/legacy-id?section=versions")).toBe("/playbooks/schedules");
    expect(playbookReturnHref("//example.com/playbooks/jobs")).toBe("/playbooks/schedules");
  });
});
