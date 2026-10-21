import { API_BASE_URL, fetchWithSession } from "@/shared/api/client";

export interface FullBackupColumn {
  name: string;
  dataType: string;
}

export interface FullBackupSkippedItem {
  scope: string;
  name: string;
  reason: string;
}

export interface FullBackupTable {
  name: string;
  section: FullBackupSection;
  columns: FullBackupColumn[];
  rows: unknown[];
}

export type FullBackupSection =
  | "inventory"
  | "connections"
  | "settings"
  | "access"
  | "secrets"
  | "automation"
  | "synchronization"
  | "observability"
  | "assistant"
  | "licensing"
  | "appearance";

export interface FullBackupSelection {
  sections?: FullBackupSection[];
  tables?: string[];
}

export type FullBackupImportAction = "replace" | "delete" | "skip";
export type FullBackupNullPolicy = "apply" | "preserveExisting";
export type FullBackupJsonMergeMode = "shallowObject";

export interface FullBackupJsonMergePolicy {
  mode: FullBackupJsonMergeMode;
  preserveKeys?: string[];
}

export interface FullBackupTablePolicy {
  action?: FullBackupImportAction | null;
  updateColumns?: string[];
  preserveColumns?: string[];
  nullPolicy?: FullBackupNullPolicy;
  jsonMergeColumns?: Record<string, FullBackupJsonMergePolicy>;
}

export interface FullBackupImportRules {
  defaultAction: FullBackupImportAction;
  sections?: Partial<Record<FullBackupSection, FullBackupImportAction>>;
  tables?: Record<string, FullBackupImportAction>;
  tablePolicies?: Record<string, FullBackupTablePolicy>;
}

export interface FullBackupPlanWarning {
  scope: string;
  name: string;
  code: string;
  message: string;
}

export interface FullBackupAppliedTablePolicy {
  updateColumns: string[];
  preserveColumns: string[];
  nullPolicy: FullBackupNullPolicy;
  jsonMergeColumns: string[];
}

export interface FullBackupTablePlan {
  name: string;
  section: FullBackupSection;
  action: FullBackupImportAction;
  rows: number;
  inserts: number;
  updates: number;
  deletes: number;
  unchanged: number;
  policy: FullBackupAppliedTablePolicy;
  warnings: FullBackupPlanWarning[];
}

export interface FullBackupImportPlan {
  dryRun: boolean;
  tableCount: number;
  rowCount: number;
  hasDestructiveChanges: boolean;
  warnings: FullBackupPlanWarning[];
  tables: FullBackupTablePlan[];
}

export interface FullBackupArchive {
  version: string;
  schemaVersion?: number | null;
  createdAt: string;
  includeSecrets: boolean;
  tableCount: number;
  rowCount: number;
  skipped: FullBackupSkippedItem[];
  tables: FullBackupTable[];
}

export interface FullBackupImportSummary {
  applied: boolean;
  tableCount: number;
  rowCount: number;
  mode: "full" | "selective";
  plan?: FullBackupImportPlan | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

async function parseError(response: Response): Promise<Error> {
  try {
    const body = (await response.clone().json()) as Record<string, unknown>;
    const message = typeof body.message === "string" ? body.message : response.statusText;
    return new Error(message || "Request failed");
  } catch {
    return new Error(response.statusText || "Request failed");
  }
}

function parseBackupArchive(value: unknown): FullBackupArchive {
  if (!isRecord(value) || value.version !== "piping-full-backup-v1" || !Array.isArray(value.tables)) {
    throw new Error("Backup file must contain a Piping full backup archive");
  }
  return value as unknown as FullBackupArchive;
}

export async function readBackupFile(file: File): Promise<FullBackupArchive> {
  return parseBackupArchive(JSON.parse(await file.text()));
}

export async function exportFullBackup(input: {
  includeSecrets: boolean;
  selection?: FullBackupSelection;
}): Promise<FullBackupArchive> {
  const response = await fetchWithSession(`${API_BASE_URL}/api/v1/admin/backup/export`, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      includeSecrets: input.includeSecrets,
      confirmSecretExport: input.includeSecrets,
      selection: input.selection,
    }),
  });
  if (!response.ok) throw await parseError(response);
  return parseBackupArchive(await response.json());
}

export async function importFullBackup(input: {
  payload: FullBackupArchive;
  selection?: FullBackupSelection;
  rules?: FullBackupImportRules;
  dryRun?: boolean;
  confirmFullRestore?: boolean;
}): Promise<FullBackupImportSummary> {
  const response = await fetchWithSession(`${API_BASE_URL}/api/v1/admin/backup/import`, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      payload: input.payload,
      confirmFullRestore: input.confirmFullRestore ?? !input.dryRun,
      dryRun: input.dryRun ?? false,
      selection: input.selection,
      rules: input.rules,
    }),
  });
  if (!response.ok) throw await parseError(response);
  return (await response.json()) as FullBackupImportSummary;
}

export function downloadBackupFile(payload: FullBackupArchive): void {
  const blob = new Blob([JSON.stringify(payload, null, 2)], {
    type: "application/json",
  });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `piping-backup-${new Date().toISOString().slice(0, 10)}.json`;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}
