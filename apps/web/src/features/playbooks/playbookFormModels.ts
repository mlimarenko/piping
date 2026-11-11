import type {
  PlaybookScheduleConcurrencyPolicy,
  PlaybookScheduleMisfirePolicy,
} from "@/features/playbooks/api";

export type TargetSetMode = "dynamic" | "static";

export type ScheduleForm = {
  name: string;
  jobTemplateId: string;
  selectionMode: "static" | "dynamic";
  targetSetId: string;
  targetsJson: string;
  cronExpression: string;
  timezone: string;
  extraVars: string;
  concurrencyPolicy: PlaybookScheduleConcurrencyPolicy;
  misfirePolicy: PlaybookScheduleMisfirePolicy;
  enabled: boolean;
};

export type ExecutionEnvironmentForm = {
  name: string;
  image: string;
  description: string;
  pullPolicy: string;
  runnerGroup: string;
  enabled: boolean;
};

export type JobTemplateForm = {
  name: string;
  projectId: string;
  playbookPath: string;
  revisionPolicy: string;
  revision: string;
  workingDirectory: string;
  inventoryMode: "managed" | "native";
  inventory: string;
  sshRecordId: string;
  argumentsText: string;
  extraVars: string;
  varsSchema: string;
  varsSchemaKind: "none" | "inline" | "project_file";
  varsSchemaPath: string;
  environment: string;
  secretReferences: string;
  limit: string;
  timeoutSeconds: string;
  targetSetId: string;
  executionEnvironmentId: string;
};

export const emptyScheduleForm = (): ScheduleForm => ({
  name: "",
  jobTemplateId: "",
  selectionMode: "static",
  targetSetId: "",
  targetsJson: "",
  cronExpression: "0 3 * * *",
  timezone: "UTC",
  extraVars: "{}",
  concurrencyPolicy: "skip_if_running",
  misfirePolicy: "fire_once",
  enabled: true,
});

export const emptyExecutionEnvironmentForm = (): ExecutionEnvironmentForm => ({
  name: "",
  image: "piping-ansible-runner:local",
  description: "",
  pullPolicy: "if_not_present",
  runnerGroup: "default",
  enabled: true,
});

export const emptyJobTemplateForm = (): JobTemplateForm => ({
  name: "",
  projectId: "",
  playbookPath: "[]",
  revisionPolicy: "current",
  revision: "",
  workingDirectory: ".",
  inventoryMode: "managed",
  inventory: "[]",
  sshRecordId: "",
  argumentsText: "[]",
  extraVars: "{}",
  varsSchema: '{"type":"object","properties":{},"additionalProperties":false}',
  varsSchemaKind: "none",
  varsSchemaPath: "",
  environment: "{}",
  secretReferences: "{}",
  limit: "",
  timeoutSeconds: "3600",
  targetSetId: "",
  executionEnvironmentId: "",
});

export const concurrencyPolicies: PlaybookScheduleConcurrencyPolicy[] = [
  "skip_if_running",
  "queue_pending",
  "replace_pending",
];
