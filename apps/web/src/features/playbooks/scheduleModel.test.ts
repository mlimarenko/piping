import { describe, expect, it } from "vitest";
import { emptyScheduleForm } from "./playbookFormModels";
import { fixedSnapshotTargets, scheduleForm, scheduleRequest, scheduleTargets, scheduleTargetsError, snapshotDiff, updateScheduleRequest, validateScheduleDraft } from "./scheduleModel";

describe("schedule draft validation", () => {
  it("saves parameter overrides for create and update without changing their types", () => {
    const values = { name: "", count: 0, enabled: false, nullable: null, items: [1, 2], nested: { x: "y" } };
    const form = { ...emptyScheduleForm(), name: "Nightly", jobTemplateId: "job", targetsJson: '[{"hostname":"node","target_id":"11111111-1111-4111-8111-111111111111"}]', servicePrincipalId: "executor", extraVars: JSON.stringify(values) };
    expect(scheduleRequest(form).overrides).toEqual({ extra_vars: values });
    expect(updateScheduleRequest(form).overrides).toEqual({ extra_vars: values });
    expect(validateScheduleDraft({ ...form, extraVars: "[]" })).toEqual(["parameters"]);
  });
  it("requires a job, executor, targets, a five-field cron and a real timezone", () => {
    const form = { ...emptyScheduleForm(), name: "Nightly", jobTemplateId: "job", targetsJson: '[{"hostname":"node","target_id":"11111111-1111-4111-8111-111111111111"}]' };
    expect(validateScheduleDraft(form)).toEqual([]);
    expect(validateScheduleDraft({ ...form, jobTemplateId: "", timezone: "Not/AZone", cronExpression: "* *" })).toHaveLength(1);
  });

  it("reports an invalid target selection as its own draft error", () => {
    const form = { ...emptyScheduleForm(), name: "Nightly", jobTemplateId: "job", targetsJson: "{" };
    expect(validateScheduleDraft(form)).toEqual(["targets"]);
  });
});

describe("schedule target validation", () => {
  const form = { ...emptyScheduleForm(), name: "Nightly", jobTemplateId: "job" };

  it("distinguishes broken JSON from a wrong shape", () => {
    expect(scheduleTargetsError({ ...form, targetsJson: "{" })).toBe("jsonSyntax");
    expect(scheduleTargetsError({ ...form, targetsJson: "[]" })).toBe("emptyFixed");
    expect(scheduleTargetsError({ ...form, selectionMode: "dynamic", targetsJson: "[]" })).toBe("selectorShape");
    expect(scheduleTargetsError({ ...form, targetsJson: '[{"hostname":"node"}]' })).toBe("fixedShape");
  });

  it("accepts a resolved fixed snapshot and a dynamic selector object", () => {
    expect(scheduleTargetsError({ ...form, targetsJson: '[{"hostname":"node","target_id":"11111111-1111-4111-8111-111111111111"}]' })).toBeNull();
    expect(scheduleTargetsError({ ...form, selectionMode: "dynamic", targetsJson: "{}" })).toBeNull();
  });
});

describe("schedule snapshot", () => {
  const targetA = { hostname: "node-a", target_id: "11111111-1111-4111-8111-111111111111", source_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa" };
  const targetB = { hostname: "node-b", target_id: "22222222-2222-4222-8222-222222222222", source_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb" };
  const targetC = { hostname: "node-c", target_id: "33333333-3333-4333-8333-333333333333", source_id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc" };

  it("reads the frozen host list of a fixed draft", () => {
    const form = { ...emptyScheduleForm(), selectionMode: "static" as const, targetsJson: JSON.stringify([targetA]) };
    expect(fixedSnapshotTargets(form)).toEqual([targetA]);
    expect(fixedSnapshotTargets({ ...form, selectionMode: "dynamic", targetsJson: "{}" })).toBeNull();
    expect(fixedSnapshotTargets({ ...form, targetsJson: "{" })).toBeNull();
  });

  it("lists added and removed hosts by identity", () => {
    expect(snapshotDiff([targetA, targetB], [targetB, targetC])).toEqual({ added: [targetC], removed: [targetA] });
  });

  it("reports an unchanged composition and survives consecutive refreshes", () => {
    expect(snapshotDiff([targetA], [targetA])).toEqual({ added: [], removed: [] });
    expect(snapshotDiff([targetB, targetC], [targetA])).toEqual({ added: [targetA], removed: [targetB, targetC] });
  });
});

describe("schedule target set provenance", () => {
  const targetSetId = "22222222-2222-4222-8222-222222222222";
  const fixedTargets = '[{"hostname":"node","target_id":"11111111-1111-4111-8111-111111111111"}]';

  it("carries the saved target set into static and dynamic selections", () => {
    const fixed = { ...emptyScheduleForm(), targetSetId, selectionMode: "static" as const, targetsJson: fixedTargets };
    expect(scheduleTargets(fixed)).toMatchObject({ kind: "fixed", target_set_id: targetSetId });
    const dynamic = { ...emptyScheduleForm(), targetSetId, selectionMode: "dynamic" as const, targetsJson: "{}" };
    expect(scheduleTargets(dynamic)).toMatchObject({ kind: "dynamic", target_set_id: targetSetId });
  });

  it("drops the reference once the selection is edited by hand", () => {
    const form = { ...emptyScheduleForm(), targetSetId: "", selectionMode: "static" as const, targetsJson: fixedTargets };
    expect(scheduleTargets(form)).not.toHaveProperty("target_set_id");
  });

  it("restores the reference from a saved schedule", () => {
    const form = scheduleForm({
      id: "33333333-3333-4333-8333-333333333333",
      name: "Nightly",
      template_id: "44444444-4444-4444-8444-444444444444",
      targets: { kind: "dynamic", selector: {}, target_set_id: targetSetId },
            owner_user_id: "66666666-6666-4666-8666-666666666666",
      cron_expression: "0 0 * * *",
      timezone: "UTC",
      concurrency_policy: "skip_if_running",
      misfire_policy: "fire_once",
      enabled: false,
      metadata: {},
      overrides: { extra_vars: { count: 0, enabled: false, nullable: null } },
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    });
    expect(form.targetSetId).toBe(targetSetId);
    expect(JSON.parse(form.extraVars)).toEqual({ count: 0, enabled: false, nullable: null });
  });
});
