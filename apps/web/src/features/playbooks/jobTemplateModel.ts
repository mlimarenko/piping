import type { CreateJobTemplateRequest, JobTemplate } from "./api";
import { emptyJobTemplateForm, type JobTemplateForm } from "./playbookFormModels";
import { isReservedEnvName } from "./secretsPolicy";
import { z } from "zod";
import { decodeAnsibleOptions } from "./ansibleOptions";
import { effectiveParameters, isObject, parameterErrors, parseParameterValues } from "./varsSchema";

export const jobDraftErrorDefaults = {
  invalid: "Complete the required fields and enter valid JSON and a positive timeout.",
  nameRequired: "Name is required.",
  projectRequired: "Select a project.",
  entrypointsInvalid: "Entrypoints must be project-relative paths without parent traversal.",
  workdirInvalid: "Working directory must be project-relative without parent traversal.",
  revisionRequired: "Select a fixed revision.",
  inventoryInvalid: "Native inventory must be a project-relative path without parent traversal.",
  sshRecordInvalid: "SSH record must be a selected record identifier.",
  targetSetRequired: "Select a managed inventory target set.",
  argumentsInvalid: "Use the supported Ansible options; raw inventory, variables, limit and credential flags are not allowed.",
  environmentInvalid: "Environment must map shell-safe variable names to values and must not shadow secret references.",
  extraVarsInvalid: "Extra variables must be a JSON object.",
  varsSchemaInvalid: "Enter an object JSON Schema for the declared parameters.",
  secretReferencesInvalid: "Secret references must map shell-safe variable names to secret IDs.",
  timeoutInvalid: "Timeout must be a positive integer.",
  executionEnvironmentRequired: "Select an execution environment.",
} as const;
export type JobDraftError = keyof typeof jobDraftErrorDefaults;

export type JobDraftField =
  | "name"
  | "project"
  | "entrypoints"
  | "workdir"
  | "revision"
  | "inventory"
  | "sshRecord"
  | "targetSet"
  | "arguments"
  | "environment"
  | "extraVars"
  | "varsSchema"
  | "secretReferences"
  | "timeout"
  | "executionEnvironment";

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const ENV_NAME_MAX_CHARS = 128;

/** Parse a JSON array of strings, or null when the text is not exactly that. */
export function stringArrayOrNull(text: string): string[] | null {
  try {
    const value: unknown = JSON.parse(text);
    return Array.isArray(value) && value.every((item): item is string => typeof item === "string") ? value : null;
  } catch {
    return null;
  }
}

function jsonValueOrNull(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

/** Parse a JSON object of string values, or null when the text is not exactly that. */
export function stringMapOrNull(text: string): Record<string, string> | null {
  const value = jsonValueOrNull(text);
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const entries: [string, string][] = [];
  for (const [key, item] of Object.entries(value)) {
    if (typeof item !== "string") return null;
    entries.push([key, item]);
  }
  return Object.fromEntries(entries);
}

export function formatStringArray(value: string[]): string {
  return JSON.stringify(value, null, 2);
}

export function formatStringMap(value: Record<string, string>): string {
  return JSON.stringify(value, null, 2);
}

export function isProjectRelativePath(path: string): boolean {
  return !path.startsWith("/") && !path.split(/[\\/]/).includes("..");
}

function invalidEnvNames(names: string[]): boolean {
  return names.some((name) => !name || name.length > ENV_NAME_MAX_CHARS || isReservedEnvName(name) || !/^[A-Z_][A-Z0-9_]*$/.test(name));
}

/** Field-level validation shared by the structured editors, the advanced JSON inputs and the request builder. */
export function jobDraftFieldErrors(form: JobTemplateForm): Partial<Record<JobDraftField, JobDraftError>> {
  const errors: Partial<Record<JobDraftField, JobDraftError>> = {};
  if (!form.name.trim()) errors.name = "nameRequired";
  if (!form.projectId) errors.project = "projectRequired";

  const paths = stringArrayOrNull(form.playbookPath);
  if (!paths?.length || paths.some((path) => !path || !isProjectRelativePath(path))) errors.entrypoints = "entrypointsInvalid";

  if (!form.workingDirectory || !isProjectRelativePath(form.workingDirectory)) errors.workdir = "workdirInvalid";
  if (form.revisionPolicy === "fixed" && !form.revision) errors.revision = "revisionRequired";

  if (form.inventoryMode === "managed") {
    if (!form.targetSetId) errors.targetSet = "targetSetRequired";
  } else {
    const sources = stringArrayOrNull(form.inventory);
    if (!sources?.length || sources.some((path) => !path || !isProjectRelativePath(path))) errors.inventory = "inventoryInvalid";
    if (form.sshRecordId.trim() && !UUID_PATTERN.test(form.sshRecordId.trim())) errors.sshRecord = "sshRecordInvalid";
  }

  if (!decodeAnsibleOptions(form.argumentsText)) errors.arguments = "argumentsInvalid";

  const environment = stringMapOrNull(form.environment);
  const secretReferences = stringMapOrNull(form.secretReferences);
  if (!environment || invalidEnvNames(Object.keys(environment)) || (secretReferences !== null && Object.keys(environment).some((name) => name in secretReferences))) {
    errors.environment = "environmentInvalid";
  }
  const extraVars = jsonValueOrNull(form.extraVars);
  if (!extraVars || typeof extraVars !== "object" || Array.isArray(extraVars)) errors.extraVars = "extraVarsInvalid";
  const varsSchema = jsonValueOrNull(form.varsSchema);
  if (form.varsSchemaKind === "inline" && (!isObject(varsSchema) || varsSchema.type !== "object" || !isObject(varsSchema.properties))) errors.varsSchema = "varsSchemaInvalid";
  if (form.varsSchemaKind === "project_file" && (!form.varsSchemaPath || !isProjectRelativePath(form.varsSchemaPath))) errors.varsSchema = "varsSchemaInvalid";
  if (form.varsSchemaKind === "none" && isObject(extraVars) && Object.keys(extraVars).length) errors.extraVars = "extraVarsInvalid";
  if (form.varsSchemaKind === "inline" && isObject(varsSchema) && !errors.extraVars && Object.keys(parameterErrors(varsSchema, effectiveParameters(varsSchema, parseParameterValues(form.extraVars), {}), false)).length > 0) errors.extraVars = "extraVarsInvalid";
  if (!secretReferences || invalidEnvNames(Object.keys(secretReferences)) || Object.values(secretReferences).some((value) => !UUID_PATTERN.test(value))) {
    errors.secretReferences = "secretReferencesInvalid";
  }

  const timeout = Number(form.timeoutSeconds);
  if (!Number.isSafeInteger(timeout) || timeout <= 0 || timeout > 2147483647) errors.timeout = "timeoutInvalid";
  if (!form.executionEnvironmentId) errors.executionEnvironment = "executionEnvironmentRequired";
  return errors;
}

export function jobTemplateRequest(form: JobTemplateForm): CreateJobTemplateRequest {
  if (Object.keys(jobDraftFieldErrors(form)).length > 0) throw new Error("Complete all required fields");
  const paths = stringArrayOrNull(form.playbookPath)!;
  const sources = stringArrayOrNull(form.inventory)!;
  return {
    name: form.name.trim(), project_id: form.projectId, workdir: form.workingDirectory,
    playbook_paths: paths,
    revision: form.revisionPolicy === "fixed" ? { kind: "fixed", revision_id: form.revision } : { kind: "current" },
    inventory: form.inventoryMode === "managed" ? { kind: "managed", target_set_id: form.targetSetId } : { kind: "native", sources, ssh_record_id: form.sshRecordId.trim() || null },
    execution_environment_id: form.executionEnvironmentId,
    vars_schema: form.varsSchemaKind === "inline" ? { kind: "inline", schema: JSON.parse(form.varsSchema) } : form.varsSchemaKind === "project_file" ? { kind: "project_file", path: form.varsSchemaPath } : { kind: "none" },
    options: {
      argv: stringArrayOrNull(form.argumentsText)!,
      extra_vars: JSON.parse(form.extraVars),
      env: stringMapOrNull(form.environment)!,
      secret_refs: stringMapOrNull(form.secretReferences)!,
      timeout_seconds: Number(form.timeoutSeconds),
      limit: form.limit || null,
    },
  };
}

export function validateJobDraft(form: JobTemplateForm): JobDraftError[] {
  return Object.keys(jobDraftFieldErrors(form)).length > 0 ? ["invalid"] : [];
}

export function jobTemplateForm(job: JobTemplate): JobTemplateForm {
  return { ...emptyJobTemplateForm(), name: job.name, projectId: job.project_id,
    playbookPath: formatStringArray(job.playbook_paths), revisionPolicy: job.revision.kind,
    revision: job.revision.kind === "fixed" ? job.revision.revision_id : "",
    workingDirectory: job.workdir, inventoryMode: job.inventory.kind,
    targetSetId: job.inventory.kind === "managed" ? job.inventory.target_set_id : "",
    inventory: job.inventory.kind === "native" ? formatStringArray(job.inventory.sources) : "[]",
    sshRecordId: job.inventory.kind === "native" ? job.inventory.ssh_record_id ?? "" : "",
    executionEnvironmentId: job.execution_environment_id,
    argumentsText: formatStringArray(job.options.argv), extraVars: JSON.stringify(job.options.extra_vars, null, 2),
    varsSchemaKind: job.vars_schema?.kind ?? "none",
    varsSchema: JSON.stringify(job.vars_schema?.kind === "inline" ? job.vars_schema.schema : { type: "object", properties: {}, additionalProperties: false }, null, 2),
    varsSchemaPath: job.vars_schema?.kind === "project_file" ? job.vars_schema.path : "",
    environment: formatStringMap(job.options.env), secretReferences: formatStringMap(job.options.secret_refs),
    limit: job.options.limit ?? "", timeoutSeconds: String(job.options.timeout_seconds) };
}

export function jobCommandPreview(form: JobTemplateForm): string {
  const paths = stringArrayOrNull(form.playbookPath);
  if (!paths) return "ansible-playbook [invalid entrypoints] [arguments hidden]";
  return `ansible-playbook ${paths.map(() => "[entrypoint]").join(" ")} [arguments hidden]`;
}

const jobTemplateFormSchema = z.object({
  name: z.string(),
  projectId: z.string(),
  playbookPath: z.string(),
  revisionPolicy: z.string(),
  revision: z.string(),
  workingDirectory: z.string(),
  inventoryMode: z.enum(["managed", "native"]),
  inventory: z.string(),
  sshRecordId: z.string(),
  argumentsText: z.string(),
  extraVars: z.string(),
  varsSchema: z.string(),
  varsSchemaKind: z.enum(["none", "inline", "project_file"]),
  varsSchemaPath: z.string(),
  environment: z.string(),
  secretReferences: z.string(),
  limit: z.string(),
  timeoutSeconds: z.string(),
  targetSetId: z.string(),
  executionEnvironmentId: z.string(),
});

/** Draft carried through the target-set create flow: the edited form and its dirty baseline. */
const jobTemplateReturnFlowDraftSchema = z.object({
  form: jobTemplateFormSchema,
  baseline: jobTemplateFormSchema,
});

export type JobTemplateReturnFlowDraft = z.infer<typeof jobTemplateReturnFlowDraftSchema>;

export function jobTemplateReturnFlowDraft(value: unknown): JobTemplateReturnFlowDraft | null {
  const parsed = jobTemplateReturnFlowDraftSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
}
