import { describe, expect, it } from "vitest";
import { matchPath } from "react-router";
import { PLAYBOOK_TAB_ROUTES, playbookRunGroupHref, playbooksTabFromPath } from "./playbookNavigation";
import { JOB_TEMPLATE_EDITOR_EDIT_PATH, jobTemplateEditorEditHref } from "./playbookEditorRoutes";
import { visibleNavigation } from "@/app/navigation";
import type { PermissionsValue } from "@/shared/auth/permissionsContext";

describe("automation navigation", () => {
  it("offers assignment settings to an assignment-only reader", () => {
    const permissions: PermissionsValue = { roles: [], hasRole: () => false, can: (action, resource) => action === "READ" && resource === "PLAYBOOK_ASSIGNMENT", canAny: () => false, isAdmin: false };
    expect(visibleNavigation(permissions).flatMap(section => section.items).some(item => item.path === PLAYBOOK_TAB_ROUTES.assignments)).toBe(true);
    expect(playbooksTabFromPath(PLAYBOOK_TAB_ROUTES.assignments)).toBe("assignments");
  });
  it("routes job edits and keeps run selection in the URL", () => {
    const edit = jobTemplateEditorEditHref("job-123");
    expect(matchPath(JOB_TEMPLATE_EDITOR_EDIT_PATH, edit)?.params.jobId).toBe("job-123");
    const run = new URL(playbookRunGroupHref("group-1", "Deploy"), "https://example.invalid");
    expect(playbooksTabFromPath(run.pathname)).toBe("runs");
    expect(run.searchParams.get("group")).toBe("group-1");
    expect(run.searchParams.get("name")).toBe("Deploy");
  });
});
