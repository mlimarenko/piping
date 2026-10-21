import { describe, expect, it } from "vitest";

import { actionCatalog, allActions, resourceTypes } from "./AccessDataContext";

describe("access control UI catalog", () => {
  it("exposes playbook permissions as first-class policy keys", () => {
    const playbookActions = actionCatalog.find((group) => group.group === "Playbooks")?.actions;

    expect(playbookActions).toEqual(["READ", "CREATE", "UPDATE", "DELETE", "EXECUTE"]);
    // `resourceTypes` is the vocabulary fallback (superseded at runtime by the
    // /access-control/vocabulary endpoint); it must cover every supported type.
    expect(resourceTypes).toEqual(
      expect.arrayContaining([
        "playbook",
        "playbook_version",
        "playbook_assignment",
        "playbook_target_set",
        "playbook_run_group",
        "playbook_run",
        "playbook_schedule",
        "playbook_artifact",
      ]),
    );
  });

  it("covers the full session-enforced resource vocabulary", () => {
    // service_principal is a session-enforced policy-editor resource type, so it
    // is a valid scope target and MUST appear.
    expect(resourceTypes).toContain("playbook_service_principal");
    // Newly-supported administrative resource types are covered too.
    expect(resourceTypes).toEqual(
      expect.arrayContaining([
        "license",
        "settings",
        "branding",
        "proxy_node",
        "monitoring",
        "backup",
      ]),
    );
    // `playbook_job_token` is an internal machine identity, never a human-scoped
    // policy resource — it stays out of the vocabulary.
    expect(resourceTypes).not.toContain("playbook_job_token");
  });

  it("deduplicates action choices used by access-check dropdowns", () => {
    expect(allActions).toEqual(Array.from(new Set(allActions)));
    expect(allActions).toEqual(expect.arrayContaining(["READ", "EXECUTE", "MANAGE_POLICIES"]));
  });
});
