import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { History, RotateCcw } from "lucide-react";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import FormSelect from "@/shared/ui-shared/FormSelect";
import EntityRef from "@/shared/ui-shared/EntityRef";
import { cn } from "@/shared/lib/utils";
import CodeEditor from "@/features/playbooks/CodeEditor";
import { diffLines } from "@/features/playbooks/lineDiff";
import { listPlaybookVersions, type PlaybookVersion } from "@/features/playbooks/api";

type Mode = "view" | "diff";

interface VersionHistoryProps {
  playbookId: string;
  currentVersionId?: string | null;
  /** When true the playbook is a read-only mirror — restore is disabled. */
  readOnly: boolean;
  /** Restore the given version's content as a new immutable version. */
  onRestore: (version: PlaybookVersion) => void;
  restoring: boolean;
}

function formatDateTime(value?: string | null) {
  if (!value) return "—";
  return new Date(value).toLocaleString();
}

/**
 * Version history for the editor dialog: lists immutable versions (DESC), lets
 * the author view one version's content read-only, diff two versions, and
 * restore an older version (which mints a new version via `onRestore`). All
 * version content is already carried in the list payload, so view/diff/restore
 * never need a per-version GET.
 */
function VersionHistory({
  playbookId,
  currentVersionId,
  readOnly,
  onRestore,
  restoring,
}: VersionHistoryProps) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<Mode>("view");
  const [selectedVersion, setSelectedVersion] = useState<number | null>(null);
  const [compareVersion, setCompareVersion] = useState<number | null>(null);

  const versionsQuery = useQuery({
    queryKey: ["playbooks", "versions", playbookId],
    queryFn: () => listPlaybookVersions(playbookId, { limit: 100, offset: 0 }),
  });

  const versions = useMemo(
    () => versionsQuery.data?.items ?? [],
    [versionsQuery.data],
  );
  const byVersion = useMemo(
    () => new Map(versions.map((v) => [v.version, v])),
    [versions],
  );

  const selected = selectedVersion != null ? byVersion.get(selectedVersion) : undefined;
  const compare = compareVersion != null ? byVersion.get(compareVersion) : undefined;

  const diff = useMemo(() => {
    if (mode !== "diff" || !selected || !compare) return null;
    // Diff older -> newer regardless of pick order.
    const [before, after] =
      compare.version < selected.version ? [compare, selected] : [selected, compare];
    return { before, after, lines: diffLines(before.content, after.content) };
  }, [mode, selected, compare]);

  if (versionsQuery.isPending) {
    return (
      <p className="py-8 text-center text-xs text-muted-foreground">{t("loading")}</p>
    );
  }
  if (versionsQuery.isError) {
    return <p role="alert">{t("unknownError")}<Button variant="outline" onClick={() => void versionsQuery.refetch()}>{t("retry")}</Button></p>;
  }
  if (versions.length === 0) {
    return (
      <p className="py-8 text-center text-xs text-muted-foreground">
        {t("playbooks.page.versions.empty")}
      </p>
    );
  }

  return (
    <div className="space-y-3">
      <div className="max-h-56 space-y-1.5 overflow-y-auto pr-1">
        {versions.map((version) => {
          const isCurrent = version.id === currentVersionId;
          const isSelected = version.version === selectedVersion;
          return (
            <div
              key={version.id}
              className={cn(
                "flex items-center justify-between gap-2 rounded-md border px-3 py-2 text-xs transition-colors",
                isSelected
                  ? "border-primary bg-accent"
                  : "border-border bg-background hover:bg-accent",
              )}
            >
              <button
                type="button"
                className="flex min-w-0 flex-1 flex-col items-start gap-1 text-left"
                onClick={() => {
                  setSelectedVersion(version.version);
                  setMode("view");
                }}
              >
                <span className="flex flex-wrap items-center gap-2">
                  <span className="font-medium text-foreground">
                    {t("playbooks.page.versions.versionNumber", { number: version.version })}
                  </span>
                  {isCurrent && (
                    <Badge variant="status-active" className="rounded-md">
                      {t("playbooks.page.versions.current")}
                    </Badge>
                  )}
                </span>
                <span className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
                  <span>{formatDateTime(version.created_at)}</span>
                  <EntityRef id={version.created_by_user_id} />
                  {version.changelog && (
                    <span className="truncate">{version.changelog}</span>
                  )}
                </span>
              </button>
              {!readOnly && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="shrink-0 gap-2"
                  disabled={restoring || isCurrent}
                  onClick={() => onRestore(version)}
                >
                  <RotateCcw className="h-3.5 w-3.5" />
                  {t("playbooks.page.versions.restore")}
                </Button>
              )}
            </div>
          );
        })}
      </div>

      {selected && (
        <div className="space-y-2 rounded-md border border-border bg-background p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-1.5 text-xs font-medium text-foreground">
              <History className="h-3.5 w-3.5 text-primary" />
              {mode === "view"
                ? t("playbooks.page.versions.viewing", { number: selected.version })
                : t("playbooks.page.versions.comparing")}
            </div>
            <div className="flex items-center gap-2">
              <FormSelect<Mode>
                value={mode}
                onValueChange={setMode}
                triggerClassName="h-9 w-28"
                options={[
                  { value: "view", label: t("playbooks.page.versions.modeView") },
                  { value: "diff", label: t("playbooks.page.versions.modeDiff") },
                ]}
              />
              {mode === "diff" && (
                <FormSelect
                  value={compareVersion != null ? String(compareVersion) : ""}
                  onValueChange={(value) => setCompareVersion(Number(value))}
                  placeholder={t("playbooks.page.versions.compareWith")}
                  triggerClassName="h-9 w-36"
                  options={versions
                    .filter((v) => v.version !== selected.version)
                    .map((v) => ({
                      value: String(v.version),
                      label: t("playbooks.page.versions.versionNumber", { number: v.version }),
                    }))}
                />
              )}
            </div>
          </div>

          {mode === "view" ? (
            <CodeEditor
              value={selected.content}
              onChange={() => undefined}
              readOnly
              minHeight="12rem"
              aria-label={t("playbooks.page.versions.viewing", {
                number: selected.version,
              })}
            />
          ) : diff ? (
            <DiffView lines={diff.lines} />
          ) : (
            <p className="py-6 text-center text-xs text-muted-foreground">
              {t("playbooks.page.versions.pickCompare")}
            </p>
          )}
        </div>
      )}
    </div>
  );
}

function DiffView({ lines }: { lines: ReturnType<typeof diffLines> }) {
  return (
    <pre className="max-h-72 overflow-auto rounded-md border border-border bg-background p-2 font-mono text-sm leading-relaxed">
      {lines.map((line, index) => {
        const sign = line.op === "added" ? "+" : line.op === "removed" ? "-" : " ";
        return (
          <div
            // Line identity is positional; index is the stable key here.
            key={index}
            className={cn(
              "whitespace-pre-wrap",
              line.op === "added" && "bg-status-positive/15 text-status-positive",
              line.op === "removed" && "bg-status-negative/15 text-status-negative",
              line.op === "equal" && "text-muted-foreground",
            )}
          >
            {sign} {line.text}
          </div>
        );
      })}
    </pre>
  );
}

export default VersionHistory;
