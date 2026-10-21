import type {
  PlaybookCatalogItem,
  PlaybookConnection,
  PlaybookRisk,
  PlaybookStatus,
} from "@/features/playbooks/api";
import { parseVarsSchema, type VarSpec } from "@/features/playbooks/varsSchema";
import { readSecretEnv, type SecretEnvRow } from "@/features/playbooks/secretsPolicy";

export const NO_TARGET_SET = "__none__";

export const DEFAULT_PLAYBOOK_CONTENT = `---
- name: Piping playbook bootstrap
  hosts: all
  gather_facts: true
  tasks:
    - name: Report the target hostname
      ansible.builtin.debug:
        msg: "Running on {{ inventory_hostname }}"
`;

export const playbookEditorSlugify = (value: string) =>
  value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "");

export interface PlaybookEditorFormState {
  name: string;
  slug: string;
  description: string;
  category: string;
  risk: PlaybookRisk;
  status: PlaybookStatus;
  connectionDefault: PlaybookConnection;
  timeoutSeconds: string;
  defaultTargetSetId: string;
  varsSchema: VarSpec[];
  /** Declared `ENV_NAME -> secret key` rows, serialized to
   *  `token_policy.runtime.secrets.env` on save (versioned). */
  secretsEnv: SecretEnvRow[];
  content: string;
  changelog: string;
}

export function formFromPlaybookItem(
  item: PlaybookCatalogItem | null,
): PlaybookEditorFormState {
  if (!item) {
    return {
      name: "",
      slug: "",
      description: "",
      category: "inventory",
      risk: "low",
      status: "draft",
      connectionDefault: "ssh",
      timeoutSeconds: "900",
      defaultTargetSetId: NO_TARGET_SET,
      varsSchema: [],
      secretsEnv: [],
      content: DEFAULT_PLAYBOOK_CONTENT,
      changelog: "",
    };
  }
  const pb = item.playbook;
  return {
    name: pb.name,
    slug: pb.slug,
    description: pb.description ?? "",
    category: pb.category,
    risk: pb.risk,
    status: pb.status,
    connectionDefault: pb.connection_default,
    timeoutSeconds: String(item.current_version?.timeout_seconds ?? 900),
    defaultTargetSetId: pb.default_target_set_id ?? NO_TARGET_SET,
    varsSchema: parseVarsSchema(pb.vars_schema),
    // Secret declarations live on the version's opaque token_policy.
    secretsEnv: readSecretEnv(item.current_version?.token_policy),
    content: item.current_version?.content ?? "",
    changelog: "",
  };
}

export function createPlaybookCopyDraft(
  item: PlaybookCatalogItem,
  copyName: string,
): PlaybookEditorFormState {
  const draft = formFromPlaybookItem(item);
  return {
    ...draft,
    name: copyName,
    slug: playbookEditorSlugify(copyName),
    status: "draft",
    defaultTargetSetId: NO_TARGET_SET,
    changelog: "",
  };
}

export type PlaybookEditorInitialTab = "editor" | "history";

export type PlaybookEditorTarget =
  | { mode: "create" }
  | { mode: "edit"; item: PlaybookCatalogItem; initialTab?: PlaybookEditorInitialTab }
  | { mode: "view"; item: PlaybookCatalogItem; initialTab?: PlaybookEditorInitialTab }
  | { mode: "copyDraft"; source: PlaybookCatalogItem; draft: PlaybookEditorFormState };
