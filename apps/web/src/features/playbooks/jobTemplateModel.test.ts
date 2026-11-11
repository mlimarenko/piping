import { describe, expect, it } from "vitest";
import { emptyJobTemplateForm } from "./playbookFormModels";
import { jobCommandPreview, jobDraftFieldErrors, jobTemplateRequest, validateJobDraft } from "./jobTemplateModel";

describe("job command display", () => {
  it("never displays arguments or variable and reference values", () => {
    const preview = jobCommandPreview({ ...emptyJobTemplateForm(), playbookPath: '["it\u0027s.yml", "other.yml"]', inventoryMode: "native", inventory: '["my inventory"]', argumentsText: "--password=private", extraVars: '{"password":"private"}', secretReferences: "private" });
    expect(preview).toContain("[entrypoint]");
    expect(preview).not.toContain("private");
    expect(preview).toContain("[arguments hidden]");
  });
  it("validates project paths, pinned revisions, inventory, JSON and numeric options", () => {
    const valid = { ...emptyJobTemplateForm(), name: "Deploy", projectId: "p", targetSetId: "t", executionEnvironmentId: "e", playbookPath: '["site.yml", "roles/update.yml"]' };
    expect(validateJobDraft(valid)).toEqual([]);
    const errors = validateJobDraft({ ...valid, playbookPath: '["../site.yml"]', workingDirectory: "/etc", revisionPolicy: "fixed", inventoryMode: "native", inventory: '["../hosts"]', timeoutSeconds: "0" });
    expect(errors).toHaveLength(1);
  });

  it("maps structured arguments, environment and secret references into the request", () => {
    const secretId = "22222222-2222-4222-8222-222222222222";
    const form = {
      ...emptyJobTemplateForm(),
      name: "Deploy",
      projectId: "11111111-1111-4111-8111-111111111111",
      targetSetId: "33333333-3333-4333-8333-333333333333",
      executionEnvironmentId: "44444444-4444-4444-8444-444444444444",
      playbookPath: '["site.yml"]',
      argumentsText: '["-v", "--check"]',
      environment: '{"FOO":"bar"}',
      secretReferences: `{"TOKEN":"${secretId}"}`,
    };
    expect(jobDraftFieldErrors(form)).toEqual({});
    const request = jobTemplateRequest(form);
    expect(request.options.argv).toEqual(["-v", "--check"]);
    expect(request.options.env).toEqual({ FOO: "bar" });
    expect(request.options.secret_refs).toEqual({ TOKEN: secretId });
  });

  it("reports invalid structured arguments, environment and secret references at their fields", () => {
    const form = {
      ...emptyJobTemplateForm(),
      name: "Deploy",
      projectId: "11111111-1111-4111-8111-111111111111",
      targetSetId: "33333333-3333-4333-8333-333333333333",
      executionEnvironmentId: "44444444-4444-4444-8444-444444444444",
      playbookPath: '["site.yml"]',
      argumentsText: "not-json",
      environment: '{"1BAD":"x"}',
      secretReferences: '{"TOKEN":"not-a-uuid"}',
    };
    expect(jobDraftFieldErrors(form)).toMatchObject({
      arguments: "argumentsInvalid",
      environment: "environmentInvalid",
      secretReferences: "secretReferencesInvalid",
    });
  });
});
