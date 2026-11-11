import { useQuery } from "@tanstack/react-query";
import { getJobTemplateVarsSchema, type JobTemplate } from "./api";

export const jobVarsSchemaQueryKey = (job: JobTemplate | undefined) => ["playbooks", "job-templates", job?.id, "vars-schema", job?.revision, job?.vars_schema] as const;

export function useJobVarsSchema(job: JobTemplate | undefined, enabled = true) {
  return useQuery({
    queryKey: jobVarsSchemaQueryKey(job),
    queryFn: () => getJobTemplateVarsSchema(job!.id),
    enabled: enabled && job?.vars_schema?.kind === "project_file",
  });
}
