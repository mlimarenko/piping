import { useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  Archive,
  Database,
  Download,
  Eye,
  FileJson,
  FileUp,
  KeyRound,
  Loader2,
  Upload,
} from "lucide-react";

import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import FormSection from "@/shared/ui-shared/FormSection";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { Button } from "@/shared/ui-primitives/button";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { notify } from "@/shared/lib/notifications";
import {
  downloadBackupFile,
  exportFullBackup,
  importFullBackup,
  readBackupFile,
  type FullBackupArchive,
  type FullBackupImportAction,
  type FullBackupImportRules,
  type FullBackupImportSummary,
  type FullBackupSection,
  type FullBackupTablePlan,
} from "@/features/backup/backup-api";

const backupSections = [
  "inventory",
  "connections",
  "settings",
  "access",
  "secrets",
  "automation",
  "synchronization",
  "observability",
  "assistant",
  "licensing",
  "appearance",
] as const;

const defaultExportSections = [...backupSections];

function formatFileSize(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function getArchiveSectionStats(archive: FullBackupArchive | null) {
  const stats = new Map<FullBackupSection, { tables: number; rows: number }>();
  for (const table of archive?.tables ?? []) {
    const section = table.section;
    const current = stats.get(section) ?? { tables: 0, rows: 0 };
    current.tables += 1;
    current.rows += table.rows.length;
    stats.set(section, current);
  }
  return stats;
}

function defaultImportActions(archive: FullBackupArchive): Partial<Record<FullBackupSection, FullBackupImportAction>> {
  const actions: Partial<Record<FullBackupSection, FullBackupImportAction>> = {};
  for (const section of getArchiveSectionStats(archive).keys()) {
    actions[section] = "replace";
  }
  return actions;
}

function toggleSection(sections: FullBackupSection[], section: FullBackupSection, checked: boolean) {
  if (checked) return Array.from(new Set([...sections, section]));
  return sections.filter((candidate) => candidate !== section);
}

function buildImportRules(
  importActions: Partial<Record<FullBackupSection, FullBackupImportAction>>,
): FullBackupImportRules {
  return {
    defaultAction: "skip",
    sections: importActions,
  };
}

function importRequestKey(input: {
  revision: number;
  file: File | null;
  selectedImportSections: FullBackupSection[];
  importActions: Partial<Record<FullBackupSection, FullBackupImportAction>>;
}): string {
  return JSON.stringify({
    revision: input.revision,
    fileName: input.file?.name ?? null,
    fileSize: input.file?.size ?? null,
    sections: input.selectedImportSections,
    actions: input.importActions,
  });
}

type FullBackupImportInput = Parameters<typeof importFullBackup>[0];
type PreviewBackupImportInput = FullBackupImportInput & { requestKey: string };

const BackupPage = () => {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // View-but-not-edit: export reads data (route already reached), but restoring a
  // backup mutates everything — hide the restore control without UPDATE on BACKUP.
  const { can } = usePermissions();
  const canRestoreBackup = can("UPDATE", "BACKUP");
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const fileRevision = useRef(0);
  const [selectionRevision, setSelectionRevision] = useState(0);
  const currentPreviewKey = useRef<string | null>(null);
  useEffect(() => () => { fileRevision.current += 1; currentPreviewKey.current = null; }, []);
  const [tab, setTab] = useState("export");
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [exportSections, setExportSections] = useState<FullBackupSection[]>([...defaultExportSections]);
  const [file, setFile] = useState<File | null>(null);
  const [archive, setArchive] = useState<FullBackupArchive | null>(null);
  const [importActions, setImportActions] = useState<Partial<Record<FullBackupSection, FullBackupImportAction>>>({});
  const [confirmRestore, setConfirmRestore] = useState(false);
  const [previewSummary, setPreviewSummary] = useState<FullBackupImportSummary | null>(null);
  const [previewKey, setPreviewKey] = useState<string | null>(null);

  const exportMutation = useApiMutation({
    mutationFn: exportFullBackup,
    onSuccess: (payload) => {
      downloadBackupFile(payload);
      notify("success", t("backup.exportReady"), {
        description: t("backup.exportReadyDescription", {
          tables: payload.tableCount,
          rows: payload.rowCount,
        }),
      });
    },
    onError: (error) => notify("error", error.message || t("backup.exportFailed")),
  });

  const importMutation = useApiMutation({
    mutationFn: importFullBackup,
    onSuccess: (summary) => {
      void queryClient.invalidateQueries();
      notify("success", t("backup.importApplied"), {
        description: t("backup.importAppliedDescription", {
          tables: summary.tableCount,
          rows: summary.rowCount,
        }),
      });
      setConfirmRestore(false);
    },
    onError: (error) => notify("error", error.message || t("backup.importFailed")),
  });

  const previewMutation = useApiMutation<FullBackupImportSummary, PreviewBackupImportInput>({
    mutationFn: ({ requestKey: _requestKey, ...input }) => importFullBackup(input),
    onSuccess: (summary, variables) => {
      if (variables.requestKey !== currentPreviewKey.current) return;
      setPreviewSummary(summary);
      setPreviewKey(variables.requestKey);
      notify("success", t("backup.previewReady"), {
        description: t("backup.previewReadyDescription", {
          tables: summary.plan?.tableCount ?? summary.tableCount,
          rows: summary.plan?.rowCount ?? summary.rowCount,
        }),
      });
    },
    onError: (error) => notify("error", error.message || t("backup.previewFailed")),
  });

  const loadSelectedFile = async (selectedFile: File | null) => {
    const revision = ++fileRevision.current;
    setSelectionRevision(revision);
    currentPreviewKey.current = null;
    setFile(selectedFile);
    setArchive(null);
    setImportActions({});
    setConfirmRestore(false);
    setPreviewSummary(null);
    setPreviewKey(null);
    if (!selectedFile) return;
    try {
      const parsedArchive = await readBackupFile(selectedFile);
      if (revision !== fileRevision.current) return;
      setArchive(parsedArchive);
      setImportActions(defaultImportActions(parsedArchive));
    } catch (error) {
      if (revision !== fileRevision.current) return;
      notify("error", error instanceof Error ? error.message : t("backup.invalidFile"));
    }
  };

  const archiveSectionStats = getArchiveSectionStats(archive);
  const selectedImportSections = Array.from(archiveSectionStats.keys()).filter(
    (section) => (importActions[section] ?? "skip") !== "skip",
  );
  const currentImportRequestKey = importRequestKey({ revision: selectionRevision, file, selectedImportSections, importActions });
  useEffect(() => { currentPreviewKey.current = currentImportRequestKey; }, [currentImportRequestKey]);
  const importRules = buildImportRules(importActions);
  const previewIsCurrent = !!previewSummary && previewKey === currentImportRequestKey;
  const canPreview = !!archive && selectedImportSections.length > 0 && !previewMutation.isPending;
  const canRestore =
    !!archive &&
    selectedImportSections.length > 0 &&
    confirmRestore &&
    previewIsCurrent &&
    !previewMutation.isPending &&
    !importMutation.isPending;

  const previewTableColumns = useMemo<Column<FullBackupTablePlan>[]>(
    () => [
      {
        key: "name",
        label: t("backup.previewTable.name"),
        className: "text-foreground",
      },
      {
        key: "action",
        label: t("backup.previewTable.action"),
        className: "text-muted-foreground",
        render: (row) => t(`backup.actions.${row.action}`),
      },
      { key: "inserts", label: t("backup.previewTable.inserts") },
      { key: "updates", label: t("backup.previewTable.updates") },
      { key: "deletes", label: t("backup.previewTable.deletes") },
      { key: "unchanged", label: t("backup.previewTable.unchanged") },
    ],
    [t],
  );

  return (
    <PageLayout width="form" className="flex flex-col gap-6">
      <PageChrome title={t("backup.title")} description={t("backup.description")} />

      <SurfaceCard className="p-4 md:p-6">
      <Tabs value={tab} onValueChange={next => { if (!importMutation.isPending) setTab(next); }}>
        <SectionTabs
          items={[
            { value: "export", label: t("backup.exportTab"), icon: Download },
            { value: "import", label: t("backup.importTab"), icon: Upload },
          ]}
        />

        <TabsContent value="export" className="space-y-4 mt-4">
          <FormSection>
            <div className="flex items-start gap-3">
              <div className="rounded-md bg-accent p-2 text-primary">
                <Archive className="h-5 w-5" />
              </div>
              <div className="min-w-0 flex-1">
                 <h2 className="text-base font-semibold text-foreground">{t("backup.fullArchive")}</h2>
                <p className="mt-1 max-w-3xl text-sm text-muted-foreground">
                  {t("backup.fullArchiveDescription")}
                </p>
              </div>
            </div>

            <div className="mt-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
              {backupSections.map((item) => (
                <label
                  key={item}
                  className="flex items-start gap-2 rounded-md border border-border/50 bg-card px-3 py-2"
                >
                  <Checkbox
                    checked={exportSections.includes(item)}
                    onCheckedChange={(value) => setExportSections((current) => toggleSection(current, item, !!value))}
                    className="mt-0.5"
                  />
                  <span className="min-w-0">
                    <span className="block text-sm font-medium text-foreground">
                      {t(`backup.coverage.${item}`)}
                    </span>
                    <span className="mt-1 block text-xs text-muted-foreground">
                      {t(`backup.coverageHint.${item}`)}
                    </span>
                  </span>
                </label>
              ))}
            </div>

            <label className="mt-4 flex items-start gap-3 rounded-md border border-border/60 bg-card p-3">
              <Checkbox
                checked={includeSecrets}
                onCheckedChange={(value) => setIncludeSecrets(!!value)}
                className="mt-0.5"
              />
              <span className="min-w-0">
                <span className="flex items-center gap-2 text-sm text-foreground">
                  <KeyRound className="h-3.5 w-3.5 text-status-warning" />
                  {t("backup.includeSecrets")}
                </span>
                <span className="mt-1 block text-sm text-muted-foreground">
                  {t("backup.includeSecretsHint")}
                </span>
              </span>
            </label>

            <div className="mt-4 flex items-center gap-3">
              <Button
                disabled={exportMutation.isPending || exportSections.length === 0}
                onClick={() =>
                  exportMutation.mutate({
                    includeSecrets,
                    selection: { sections: exportSections },
                  })
                }
              >
                {exportMutation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Download className="h-3.5 w-3.5" />
                )}
                {t("backup.generate")}
              </Button>
              <span className="text-sm text-muted-foreground">{t("backup.formatHint")}</span>
            </div>
          </FormSection>
        </TabsContent>

        <TabsContent value="import" className="space-y-4 mt-4">
          <FormSection>
            <input
              ref={fileInputRef}
              type="file"
              accept=".json,application/json"
              disabled={importMutation.isPending}
              className="hidden"
              onChange={(event) => void loadSelectedFile(event.target.files?.[0] ?? null)}
            />
            <div className="rounded-lg border-2 border-dashed border-border/60 p-6 text-center">
              <FileUp className="mx-auto mb-2 h-8 w-8 text-muted-foreground" />
              {file ? (
                <div className="mb-1 flex items-center justify-center gap-2">
                  <FileJson className="h-4 w-4 text-primary" />
                  <span className="text-sm font-medium text-foreground">{file.name}</span>
                  <span className="text-sm text-muted-foreground">({formatFileSize(file.size)})</span>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-9 px-3 text-sm"
                    aria-label={t("cancel")}
                    disabled={importMutation.isPending}
                    onClick={() => void loadSelectedFile(null)}
                  >
                    x
                  </Button>
                </div>
              ) : (
                <>
                  <p className="mb-1 text-sm text-foreground">{t("backup.dropFile")}</p>
                  <p className="text-sm text-muted-foreground">{t("backup.acceptsJson")}</p>
                </>
              )}
              <Button
                variant="outline"
                size="sm"
                className="mt-2 h-9 text-sm"
                disabled={importMutation.isPending}
                onClick={() => fileInputRef.current?.click()}
              >
                {t("backup.chooseFile")}
              </Button>
            </div>
          </FormSection>

          {archive ? (
            <FormSection>
              <div className="mb-3 flex items-center justify-between gap-3">
                <div>
                  <h2 className="text-base font-semibold text-foreground">{t("backup.archivePreview")}</h2>
                  <p className="mt-1 text-sm text-muted-foreground">
                    {new Date(archive.createdAt).toLocaleString()}
                  </p>
                </div>
                <div className="rounded-md bg-accent px-3 py-1.5 text-sm text-foreground">
                  {archive.version}
                  {archive.schemaVersion ? ` · ${t("backup.schema")} ${archive.schemaVersion}` : ""}
                </div>
              </div>

              <div className="grid gap-2 sm:grid-cols-3">
                <div className="rounded-md border border-border/50 bg-card p-3">
                  <div className="text-xs uppercase text-muted-foreground">{t("backup.tables")}</div>
                  <div className="mt-1 text-lg font-semibold text-foreground">{archive.tableCount}</div>
                </div>
                <div className="rounded-md border border-border/50 bg-card p-3">
                  <div className="text-xs uppercase text-muted-foreground">{t("backup.rows")}</div>
                  <div className="mt-1 text-lg font-semibold text-foreground">{archive.rowCount}</div>
                </div>
                <div className="rounded-md border border-border/50 bg-card p-3">
                  <div className="text-xs uppercase text-muted-foreground">{t("backup.secrets")}</div>
                  <div className="mt-1 flex items-center gap-2 text-sm text-foreground">
                    {archive.includeSecrets ? <KeyRound className="h-4 w-4 text-status-warning" /> : <Database className="h-4 w-4" />}
                    {archive.includeSecrets ? t("backup.secretsIncluded") : t("backup.secretsExcluded")}
                  </div>
                </div>
              </div>

              {!archive.includeSecrets ? (
                <div className="mt-4 rounded-md border border-border bg-muted p-3 text-sm text-foreground">
                  <div className="mb-1 flex items-center gap-2 font-medium text-status-warning">
                    <AlertTriangle className="h-4 w-4" />
                    {t("backup.restoreBlockedTitle")}
                  </div>
                  {t("backup.restoreBlockedDescription")}
                </div>
              ) : null}

              <div className="mt-4 rounded-md border border-border/50 bg-card">
                <div className="border-b border-border/50 px-3 py-2 text-sm font-medium text-foreground">
                  {t("backup.importRules")}
                </div>
                <div className="divide-y divide-border/40">
                  {Array.from(archiveSectionStats.entries()).map(([section, stats]) => (
                    <div key={section} className="grid gap-3 px-3 py-2 sm:grid-cols-[1fr_220px] sm:items-center">
                      <div className="min-w-0">
                        <div className="text-sm font-medium text-foreground">{t(`backup.coverage.${section}`)}</div>
                        <div className="mt-0.5 text-xs text-muted-foreground">
                          {t("backup.sectionStats", { tables: stats.tables, rows: stats.rows })}
                        </div>
                      </div>
                      <FormSelect<FullBackupImportAction>
                        disabled={importMutation.isPending || !canRestoreBackup}
                        aria-label={t(`backup.coverage.${section}`)}
                        value={importActions[section] ?? "skip"}
                         onValueChange={(value) => {
                           currentPreviewKey.current = null;
                          setPreviewSummary(null);
                          setPreviewKey(null);
                          setImportActions((current) => ({
                            ...current,
                            [section]: value,
                          }));
                        }}
                        options={(["replace", "delete", "skip"] as const).map((value) => ({
                          value,
                          label: t(`backup.actions.${value}`),
                        }))}
                      />
                    </div>
                  ))}
                </div>
              </div>

              <label className="mt-4 flex items-start gap-3 rounded-md border border-border bg-muted p-3">
                <Checkbox
                  disabled={importMutation.isPending || !canRestoreBackup}
                  checked={confirmRestore}
                  onCheckedChange={(value) => setConfirmRestore(!!value)}
                  className="mt-0.5"
                />
                <span className="text-sm text-foreground">
                  <span className="block font-medium">{t("backup.confirmRestore")}</span>
                  <span className="mt-1 block text-muted-foreground">{t("backup.confirmRestoreHint")}</span>
                </span>
              </label>
              {archive.includeSecrets ? null : (
                <div className="mt-2 text-xs text-muted-foreground">{t("backup.partialWithoutSecretsHint")}</div>
              )}
            </FormSection>
          ) : null}

          {previewSummary?.plan ? (
            <FormSection>
              <div className="flex items-start justify-between gap-3">
                <div>
                  <h2 className="text-base font-semibold text-foreground">
                    {t("backup.previewTitle")}
                  </h2>
                  <p className="mt-1 text-sm text-muted-foreground">
                    {previewIsCurrent ? t("backup.previewCurrent") : t("backup.previewStale")}
                  </p>
                </div>
                {previewSummary.plan.hasDestructiveChanges ? (
                  <div className="rounded-md border border-border bg-muted px-2 py-1 text-xs text-status-negative">
                    {t("backup.destructiveChanges")}
                  </div>
                ) : null}
              </div>
              <div className="mt-3 grid gap-2 sm:grid-cols-5">
                {[
                  ["tables", previewSummary.plan.tableCount],
                  [
                    "inserts",
                    previewSummary.plan.tables.reduce((sum, table) => sum + table.inserts, 0),
                  ],
                  [
                    "updates",
                    previewSummary.plan.tables.reduce((sum, table) => sum + table.updates, 0),
                  ],
                  [
                    "deletes",
                    previewSummary.plan.tables.reduce((sum, table) => sum + table.deletes, 0),
                  ],
                  [
                    "unchanged",
                    previewSummary.plan.tables.reduce((sum, table) => sum + table.unchanged, 0),
                  ],
                ].map(([key, value]) => (
                  <div key={key} className="rounded-md border border-border/50 bg-card p-3">
                    <div className="text-xs uppercase text-muted-foreground">
                      {t(`backup.previewStats.${key}`)}
                    </div>
                    <div className="mt-1 text-lg font-semibold text-foreground">{value}</div>
                  </div>
                ))}
              </div>
              {previewSummary.plan.warnings.length > 0 ? (
                <div className="mt-3 rounded-md border border-border bg-muted p-3 text-sm text-foreground">
                  <div className="mb-1 flex items-center gap-2 font-medium text-status-warning">
                    <AlertTriangle className="h-4 w-4" />
                    {t("backup.previewWarnings")}
                  </div>
                  <ul className="space-y-1">
                    {previewSummary.plan.warnings.slice(0, 5).map((warning) => (
                      <li key={`${warning.scope}:${warning.name}:${warning.code}`}>{warning.message}</li>
                    ))}
                  </ul>
                </div>
              ) : null}
              <DataTable
                tableId="backup.preview"
                className="mt-4"
                columns={previewTableColumns}
                data={previewSummary.plan.tables}
                getRowKey={(row) => row.name}
                virtualizeRows={false}
                density="compact"
              />
            </FormSection>
          ) : null}

          {canRestoreBackup && (
            <div className="flex flex-wrap items-center gap-3">
              <Button
                variant="outline"
                disabled={!canPreview}
                onClick={() =>
                  archive &&
                  previewMutation.mutate({
                    payload: archive,
                    dryRun: true,
                    confirmFullRestore: false,
                    requestKey: currentImportRequestKey,
                    selection: { sections: selectedImportSections },
                    rules: importRules,
                  })
                }
              >
                {previewMutation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Eye className="h-3.5 w-3.5" />
                )}
                {t("backup.preview")}
              </Button>
              <Button
                disabled={!canRestore}
                onClick={() =>
                  archive &&
                  importMutation.mutate({
                    payload: archive,
                    dryRun: false,
                    selection: { sections: selectedImportSections },
                    rules: importRules,
                  })
                }
              >
                {importMutation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Upload className="h-3.5 w-3.5" />
                )}
                {t("backup.restore")}
              </Button>
              <span className="text-sm text-muted-foreground">
                {previewIsCurrent ? t("backup.restoreHint") : t("backup.previewRequiredHint")}
              </span>
            </div>
          )}
        </TabsContent>
      </Tabs>
      </SurfaceCard>
    </PageLayout>
  );
};

export default BackupPage;
