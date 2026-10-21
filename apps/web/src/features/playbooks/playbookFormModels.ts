import type {
  PlaybookAssignmentScopeKind,
  PlaybookScheduleConcurrencyPolicy,
} from "@/features/playbooks/api";

export type TargetSetMode = "dynamic" | "static";

export type ScheduleForm = {
  name: string;
  playbookId: string;
  targetSetId: string;
  servicePrincipalId: string;
  cronExpression: string;
  timezone: string;
  concurrencyPolicy: PlaybookScheduleConcurrencyPolicy;
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

export type ProjectForm = {
  name: string;
  description: string;
  scmType: string;
  scmUrl: string;
  scmBranch: string;
  scmRef: string;
  playbookBasePath: string;
  defaultExecutionEnvironmentId: string;
  syncPolicy: string;
};

export type JobTemplateSourceKind = "playbook" | "project";

export type JobTemplateForm = {
  name: string;
  description: string;
  sourceKind: JobTemplateSourceKind;
  playbookId: string;
  projectId: string;
  playbookPath: string;
  targetSetId: string;
  executionEnvironmentId: string;
  enabled: boolean;
};

export type AssignmentForm = {
  playbookId: string;
  targetSetId: string;
  name: string;
  scopeKind: PlaybookAssignmentScopeKind;
  scopeId: string;
  priority: string;
  enabled: boolean;
  visible: boolean;
  executable: boolean;
  quickActionEnabled: boolean;
};

export const emptyScheduleForm = (): ScheduleForm => ({
  name: "",
  playbookId: "",
  targetSetId: "",
  servicePrincipalId: "",
  cronExpression: "0 3 * * *",
  timezone: "UTC",
  concurrencyPolicy: "skip_if_running",
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

export const emptyProjectForm = (): ProjectForm => ({
  name: "",
  description: "",
  scmType: "manual",
  scmUrl: "",
  scmBranch: "main",
  scmRef: "",
  playbookBasePath: "",
  defaultExecutionEnvironmentId: "",
  syncPolicy: "manual",
});

export const emptyJobTemplateForm = (): JobTemplateForm => ({
  name: "",
  description: "",
  sourceKind: "playbook",
  playbookId: "",
  projectId: "",
  playbookPath: "play.yml",
  targetSetId: "",
  executionEnvironmentId: "",
  enabled: true,
});

export const concurrencyPolicies: PlaybookScheduleConcurrencyPolicy[] = [
  "skip_if_running",
  "queue_pending",
  "replace_pending",
];
