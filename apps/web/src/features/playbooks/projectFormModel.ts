import type { components } from "@/shared/api/schema.gen";

export type ProjectForm = {
  name: string;
  description: string;
  /** `system` is the product-owned built-in catalog project: read-only here. */
  sourceKind: "editable" | "git" | "system";
  url: string;
  reference: string;
  authKind: "anonymous" | "https_basic" | "https_bearer" | "ssh_record";
  username: string;
  secretId: string;
  sshRecordId: string;
};

export const emptyProjectForm = (): ProjectForm => ({ name: "", description: "", sourceKind: "editable", url: "", reference: "HEAD", authKind: "anonymous", username: "", secretId: "", sshRecordId: "" });

export function projectFormFromProject(project: components["schemas"]["Project"]): ProjectForm {
  const source = project.source;
  return { ...emptyProjectForm(), name: project.name, description: project.description ?? "", sourceKind: source.kind, ...(source.kind === "git" ? { url: source.url, reference: source.reference, authKind: source.auth.kind, username: source.auth.kind === "https_basic" ? source.auth.username : "", secretId: source.auth.kind === "https_basic" || source.auth.kind === "https_bearer" ? source.auth.secret_id : "", sshRecordId: source.auth.kind === "ssh_record" ? source.auth.ssh_record_id : "" } : {}) };
}

export function projectSource(form: ProjectForm): components["schemas"]["ProjectSource"] {
  if (form.sourceKind === "system") {
    // The built-in catalog project is read-only; producing a Git source here
    // would silently turn it into a user project.
    throw new Error("The built-in catalog project is read-only");
  }
  if (form.sourceKind === "editable") return { kind: "editable" };
  const auth: components["schemas"]["ProjectAuth"] = form.authKind === "https_basic"
    ? { kind: "https_basic", username: form.username.trim(), secret_id: form.secretId.trim() }
    : form.authKind === "https_bearer" ? { kind: "https_bearer", secret_id: form.secretId.trim() }
      : form.authKind === "ssh_record" ? { kind: "ssh_record", ssh_record_id: form.sshRecordId.trim() } : { kind: "anonymous" };
  return { kind: "git", url: form.url.trim(), reference: form.reference.trim(), auth };
}

/** The bundled catalog project: its source, files and jobs are product-owned. */
export function projectIsSystemReadOnly(project: components["schemas"]["Project"]): boolean {
  return project.source.kind === "system";
}

export function projectSettingsApplied(submitted: ProjectForm, project: components["schemas"]["Project"]): boolean {
  const returned = projectFormFromProject(project);
  return project.name === submitted.name.trim() &&
    (project.description ?? "").trim() === submitted.description.trim() &&
    JSON.stringify(projectSource(returned)) === JSON.stringify(projectSource(submitted));
}

export function validProjectForm(form: ProjectForm): boolean {
  if (!form.name.trim()) return false;
  if (form.sourceKind === "editable") return true;
  if (!form.url.trim() || !form.reference.trim()) return false;
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  if (form.authKind === "https_basic") return !!form.username.trim() && uuid.test(form.secretId.trim());
  if (form.authKind === "https_bearer") return uuid.test(form.secretId.trim());
  if (form.authKind === "ssh_record") return uuid.test(form.sshRecordId.trim());
  return true;
}
