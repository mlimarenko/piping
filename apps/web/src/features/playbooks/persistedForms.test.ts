import { describe, expect, it } from "vitest";
import type { JobTemplate, PlaybookSchedule, PlaybookTargetSetPreview } from "./api";
import { jobTemplateForm, jobTemplateRequest } from "./jobTemplateModel";
import { fixedTargetsFromPreview, scheduleForm, scheduleRequest, updateScheduleRequest } from "./scheduleModel";

const secretId = ["11111111","1111","4111","8111","111111111111"].join("-");
const sshRecordId = ["22222222","2222","4222","8222","222222222222"].join("-");
const job: JobTemplate = { id: "job", name: "Job", project_id: "project", workdir: "infra", playbook_paths: ["one.yml", "two.yml"], revision: { kind: "fixed", revision_id: "revision" }, inventory: { kind: "native", sources: ["inventory/a", "inventory/b"], ssh_record_id: sshRecordId }, vars_schema: { kind: "inline", schema: { type: "object", properties: { nested: { type: "array", items: { type: "number" } } } } }, execution_environment_id: "environment", options: { argv: ["--check", "--tags=verify"], env: { A: "value" }, extra_vars: { nested: [1] }, secret_refs: { DB_PASSWORD: secretId }, limit: "web", timeout_seconds: 42 } };
const schedule: PlaybookSchedule = { id: "schedule", template_id: "job", owner_user_id: "user", name: "Nightly", cron_expression: "0 3 * * *", timezone: "UTC", misfire_policy: "fire_once", concurrency_policy: "skip_if_running", enabled: true, targets: { kind: "dynamic", selector: { target_kind: ["unusual"], advanced: { keep: true } } }, metadata: {}, created_at: "", updated_at: "" };

describe("persisted job and schedule forms", () => {
  it("round-trips every job launch option including secret references", () => {
    const { id: _id, ...request } = job;
    expect(jobTemplateRequest(jobTemplateForm(job))).toEqual(request);
  });
  it("rejects malformed maps without silently dropping values", () => {
    expect(() => jobTemplateRequest({ ...jobTemplateForm(job), secretReferences: '{"PASS":42}' })).toThrow();
  });
  it("preserves opaque selector and does not patch immutable identities", () => {
    const form = scheduleForm(schedule);
    expect(scheduleRequest(form).targets).toEqual(schedule.targets);
    expect(updateScheduleRequest(form)).not.toHaveProperty("template_id");
    expect(updateScheduleRequest(form)).not.toHaveProperty("service_principal_id");
  });
  it("round-trips fixed resolved identities and rejects incomplete targets", () => {
    const fixed: PlaybookSchedule = { ...schedule, targets: { kind: "fixed", targets: [{ hostname: "node", target_id: "11111111-1111-4111-8111-111111111111", source_id: "22222222-2222-4222-8222-222222222222" }] } };
    expect(scheduleRequest(scheduleForm(fixed)).targets).toEqual(fixed.targets);
    expect(() => scheduleRequest({ ...scheduleForm(fixed), targetsJson: '[{"hostname": 42}]' })).toThrow();
  });
  it("freezes the complete preview and rejects truncated or invalid members", () => {
    const target = "11111111-1111-4111-8111-111111111111";
    const source = "22222222-2222-4222-8222-222222222222";
    const item = { name: "node", target_id: target, source_id: source, target_kind: "connection_target", source_kind: "device", address_bound: true };
    const preview: PlaybookTargetSetPreview = { items: [item], total: 1, limit: 1000, truncated: false, target_set: { id: "set", name: "Set", mode: "static", entity_kind: "connection_target", selector: {}, created_at: "", updated_at: "" } };
    expect(fixedTargetsFromPreview(preview)).toEqual([{ hostname: "node", target_id: target, source_id: source }]);
    expect(() => fixedTargetsFromPreview({ ...preview, truncated: true })).toThrow();
    expect(() => fixedTargetsFromPreview({ ...preview, items: [] })).toThrow();
    expect(() => fixedTargetsFromPreview({ ...preview, items: [{ ...item, name: "" }] })).toThrow();
  });
});
