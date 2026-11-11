import type { components } from "@/shared/api/schema.gen";

export function automationLaunchSpec(overrides: Partial<components["schemas"]["LaunchSpec"]> = {}): components["schemas"]["LaunchSpec"] {
  return {
    template_id: "job-1", project_id: "project-1", revision_id: "revision-1",
    tree_sha256: "0".repeat(64), execution_environment_id: "environment-1",
    image: "example/ansible", image_digest: `sha256:${"1".repeat(64)}`,
    workdir: ".", playbook_paths: ["site.yml"],
    inventory: { kind: "native", sources: ["inventory.ini"] },
    options: { argv: [], extra_vars: {}, env: {}, secret_refs: {}, timeout_seconds: 1800 },
    targets: [],
    ...overrides,
  };
}
