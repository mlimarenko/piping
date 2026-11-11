import { describe, expect, it } from "vitest";
import { decodeAnsibleOptions, emptyAnsibleOptions, encodeAnsibleOptions } from "./ansibleOptions";
import { emptyJobTemplateForm } from "./playbookFormModels";
import { jobDraftFieldErrors, jobTemplateRequest } from "./jobTemplateModel";

describe("structured Ansible options", () => {
  it("round trips supported flags and literal values without shell splitting", () => {
    const options = {
      tags: "deploy,spaces ' $(not-a-command);",
      skipTags: "slow",
      check: true,
      diff: true,
      verbosity: 4,
      become: true,
      becomeUser: "service",
      becomeMethod: "sudo",
    };
    expect(decodeAnsibleOptions(encodeAnsibleOptions(options))).toEqual(options);
    expect(decodeAnsibleOptions('["-t", "deploy", "-C", "-D", "-b", "--verbose"]')).toMatchObject({ tags: "deploy", check: true, diff: true, become: true, verbosity: 1 });
    expect(JSON.parse(encodeAnsibleOptions(emptyAnsibleOptions()))).toEqual([]);
  });

  it.each([
    ["-ihosts"], ["--inventory=hosts"], ["-e", "value=1"], ["--extra-vars=@vars.json"],
    ["--limit=localhost"], ["-lhost"], ["--ask-pass"], ["-vK"], ["--step"],
    ["--vault-id=prod@prompt"], ["--private-key=/key"], ["--connection=local"],
    ["--tags"], ["--tags="], ["--tags=a", "-t", "b"], ["--check=false"],
    ["-vvvvv"], ["--become-user=root"], ["site.yml"], ["--tags=bad\u0085value"],
  ])("rejects an unsupported or ambiguous argument set: %j", (...args) => {
    expect(decodeAnsibleOptions(JSON.stringify(args))).toBeNull();
    expect(jobDraftFieldErrors({ ...emptyJobTemplateForm(), argumentsText: JSON.stringify(args) }).arguments).toBe("argumentsInvalid");
  });

  it("keeps the platform limit and typed variables separate from argv in the saved request", () => {
    const form = {
      ...emptyJobTemplateForm(), name: "Deploy", projectId: "project", targetSetId: "targets",
      executionEnvironmentId: "environment", playbookPath: '["site.yml"]', limit: "host*:!skip",
      argumentsText: encodeAnsibleOptions({ ...emptyAnsibleOptions(), check: true, tags: "verify" }),
      varsSchemaKind: "inline" as const,
      varsSchema: JSON.stringify({ type: "object", properties: { count: { type: "number" }, enabled: { type: "boolean" }, empty: { type: "string" }, nothing: { type: ["string", "null"] } } }),
      extraVars: '{"count":0,"enabled":false,"empty":"","nothing":null}',
    };
    const request = jobTemplateRequest(form);
    expect(request.options.argv).toEqual(["--tags=verify", "--check"]);
    expect(request.options.limit).toBe("host*:!skip");
    expect(request.options.extra_vars).toEqual({ count: 0, enabled: false, empty: "", nothing: null });
    expect(() => jobTemplateRequest({ ...form, argumentsText: '["--limit=other"]' })).toThrow();
  });
});
