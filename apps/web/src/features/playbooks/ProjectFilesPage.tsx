import { useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ChevronDown, ChevronRight, FileCode2, Folder, Loader2, Plus, Trash2 } from "lucide-react";

import { useApiMutation } from "@/shared/api/hooks";
import { ApiError } from "@/shared/api/client";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { notify } from "@/shared/lib/notifications";
import { cn } from "@/shared/lib/utils";

import CodeEditor from "@/features/playbooks/CodeEditor";
import { ToggleField, FormField } from "@/features/playbooks/PlaybookFormFields";
import {
  addFile,
  buildFileTree,
  draftIssue,
  draftToUpload,
  duplicatePaths,
  filesEqual,
  filesToDraft,
  nextFilePath,
  removeFile,
  updateFile,
  type ProjectFileDraft,
  type ProjectFileTreeNode,
} from "@/features/playbooks/projectFilesModel";
import { getProject, getProjectFiles, uploadProjectFiles } from "@/features/playbooks/api";
import type { PlaybookProject } from "@/features/playbooks/api";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";

const FILES_QUERY_KEY = (id: string) => ["playbooks", "project", id, "files"] as const;

function FileTreeNode({
  node,
  selectedFileId,
  onSelect,
}: Readonly<{
  node: ProjectFileTreeNode;
  selectedFileId: string | null;
  onSelect: (fileId: string) => void;
}>) {
  const [open, setOpen] = useState(true);
  if (node.kind === "folder") {
    return (
      <li>
        <button
          type="button"
          className="flex w-full items-center gap-1 rounded-sm px-1.5 py-1 text-left text-xs text-muted-foreground hover:bg-accent/40"
          onClick={() => setOpen((prev) => !prev)}
        >
          {open ? (
            <ChevronDown className="size-3.5 shrink-0" />
          ) : (
            <ChevronRight className="size-3.5 shrink-0" />
          )}
          <Folder className="size-3.5 shrink-0" />
          <span className="truncate font-mono">{node.name}</span>
        </button>
        {open && (
          <ul className="ml-3 border-l border-border pl-2">
            {node.children.map((child) => (
              <FileTreeNode
                key={child.fileId ?? `${child.kind}:${child.path}`}
                node={child}
                selectedFileId={selectedFileId}
                onSelect={onSelect}
              />
            ))}
          </ul>
        )}
      </li>
    );
  }
  return (
    <li>
      <button
        type="button"
        aria-current={node.fileId === selectedFileId ? "true" : undefined}
        className={cn(
          "flex w-full items-center gap-1 rounded-sm px-1.5 py-1 text-left text-xs hover:bg-accent/40",
          node.fileId === selectedFileId ? "bg-accent text-foreground" : "text-muted-foreground",
        )}
        onClick={() => node.fileId && onSelect(node.fileId)}
      >
        <FileCode2 className="size-3.5 shrink-0" />
        <span className="truncate font-mono">{node.name}</span>
      </button>
    </li>
  );
}

function ProjectFilesEditor({
  project,
  initialFiles,
}: Readonly<{
  project: PlaybookProject;
  initialFiles: ProjectFileDraft[];
}>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [baseline, setBaseline] = useState<ProjectFileDraft[]>(() => initialFiles);
  const [draft, setDraft] = useState<ProjectFileDraft[]>(() => initialFiles);
  const [selectedId, setSelectedId] = useState<string | null>(initialFiles[0]?.id ?? null);
  const [error, setError] = useState("");

  const isManual = project.scm_type === "manual";
  const dirty = isManual && !filesEqual(draft, baseline);
  const duplicates = duplicatePaths(draft);
  const issue = draftIssue(draft);
  const selected = draft.find((file) => file.id === selectedId) ?? null;

  const back = () => {
    void navigate(PLAYBOOK_TAB_ROUTES.projects);
  };

  const mutation = useApiMutation({
    mutationFn: () => uploadProjectFiles(project.id, draftToUpload(draft)),
    onSuccess: () => {
      setBaseline(draft.map((file) => ({ ...file })));
      void queryClient.invalidateQueries({ queryKey: FILES_QUERY_KEY(project.id) });
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "projects"] });
      notify(
        "success",
        t("playbooks.page.projects.files.saved", { defaultValue: "Project files saved" }),
      );
    },
    onError: () => {
      setError(
        t("playbooks.page.projects.files.saveFailed", {
          defaultValue: "Could not save project files.",
        }),
      );
    },
  });

  const selectAfter = (files: ProjectFileDraft[]) => {
    setSelectedId(files[0]?.id ?? null);
  };

  const onAdd = () => {
    const path = nextFilePath(draft);
    const next = addFile(draft, path);
    setDraft(next);
    const created = next.at(-1);
    if (created) setSelectedId(created.id);
  };

  const onDelete = (id: string) => {
    const next = removeFile(draft, id);
    setDraft(next);
    if (selectedId === id) selectAfter(next);
  };

  const tree = buildFileTree(draft);

  const issueMessage = (() => {
    if (issue === "empty") {
      return t("playbooks.page.projects.files.minimumOne", {
        defaultValue: "At least one file is required.",
      });
    }
    if (issue === "blankPath") {
      return t("playbooks.page.projects.files.pathRequired", { defaultValue: "File path is required." });
    }
    if (issue === "duplicatePath") {
      return t("playbooks.page.projects.files.duplicatePath", {
        defaultValue: "Two files share this path.",
      });
    }
    return "";
  })();

  const nonManualNotice = !isManual ? (
    <p
      role="note"
      className="rounded-md border border-border bg-muted px-3 py-2 text-xs text-muted-foreground"
    >
      {t("playbooks.page.projects.files.nonManual", {
        scmType: t(`playbooks.page.projects.scmType.${project.scm_type}`, {
          defaultValue: project.scm_type ?? "SCM",
        }),
        defaultValue:
          "File management applies to manual projects only. This project is synced from {{scmType}}.",
      })}
    </p>
  ) : null;

  return (
    <EntityEditorPage scope="routed"
      width="standard"
      title={project.name}
      description={t("playbooks.page.projects.files.description", {
        defaultValue: "Files stored with this manual project. Saving replaces the whole set.",
      })}
      breadcrumbs={[
        {
          key: "projects",
          label: t("playbooks.page.projects.title"),
          onClick: back,
        },
        {
          key: "files",
          label: t("playbooks.page.projects.files.title", { defaultValue: "Project files" }),
        },
      ]}
      onBack={back}
      backLabel={t("back")}
      breadcrumbLabel={t("back")}
      onSave={isManual ? () => {
        if (issue) {
          setError(issueMessage);
          return;
        }
        setError("");
        mutation.mutate();
      } : undefined}
      saveLabel={t("save")}
      saving={mutation.isPending}
      saveDisabled={issue !== null}
      onCancel={back}
      cancelLabel={isManual ? t("cancel") : t("back")}
      dirty={dirty}
      dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}
      error={
        nonManualNotice ??
        (error ? (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        ) : undefined)
      }
    >
      <div className="flex min-h-0 flex-col gap-4 md:flex-row">
        <div className="flex min-w-0 shrink-0 flex-col gap-2 md:w-64">
          {isManual && (
            <Button type="button" variant="outline" disabled={mutation.isPending} className="gap-1.5" onClick={onAdd}>
              <Plus className="size-3.5" />
              {t("playbooks.page.projects.files.add", { defaultValue: "Add file" })}
            </Button>
          )}
          {tree.length > 0 && (
            <ul className="min-h-0 flex-1 overflow-y-auto">
              {tree.map((node) => (
                <FileTreeNode
                  key={node.fileId ?? `${node.kind}:${node.path}`}
                  node={node}
                  selectedFileId={selectedId}
                  onSelect={setSelectedId}
                />
              ))}
            </ul>
          )}
        </div>

        <div className="flex min-w-0 flex-1 flex-col gap-3 border-t border-border pt-4 md:border-t-0 md:border-l md:pt-0 md:pl-4">
          {selected ? (
            <>
              <div className="flex items-end gap-2">
                <FormField
                  label={t("playbooks.page.projects.files.path", { defaultValue: "Path" })}
                  className="min-w-0 flex-1"
                >
                  <Input
                    className="h-9 font-mono text-sm"
                    value={selected.path}
                    readOnly={!isManual}
                    disabled={mutation.isPending}
                    aria-label={t("playbooks.page.projects.files.path", { defaultValue: "Path" })}
                    aria-invalid={duplicates.has(selected.path) || undefined}
                    placeholder={t("playbooks.page.projects.files.pathPlaceholder", {
                      defaultValue: "playbooks/site.yml",
                    })}
                    onChange={(event) => {
                      if (mutation.isPending) return;
                      const next = event.target.value;
                      setDraft((current) => updateFile(current, selected.id, { path: next }));
                    }}
                  />
                </FormField>
                {isManual && (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="h-9 gap-1.5 text-destructive"
                    disabled={mutation.isPending}
                    aria-label={t("delete")}
                    onClick={() => onDelete(selected.id)}
                  >
                    <Trash2 className="size-3.5" />
                    {t("delete")}
                  </Button>
                )}
              </div>

              {issueMessage && (
                <p role="alert" className="text-xs text-destructive">
                  {issueMessage}
                </p>
              )}

              {isManual && (
                <ToggleField
                  label={t("playbooks.page.fields.executable")}
                  checked={selected.executable}
                  disabled={mutation.isPending}
                  onCheckedChange={(checked) =>
                    !mutation.isPending && setDraft((current) => updateFile(current, selected.id, { executable: checked }))
                  }
                />
              )}
              {!isManual && selected.executable && (
                <p className="text-xs text-muted-foreground">
                  {t("playbooks.page.fields.executable")}
                </p>
              )}

              <FormField
                label={t("playbooks.page.projects.files.content", { defaultValue: "Content" })}
              >
                <CodeEditor
                  value={selected.content}
                  readOnly={!isManual || mutation.isPending}
                  minHeight="20rem"
                  aria-label={t("playbooks.page.projects.files.content", {
                    defaultValue: "Content",
                  })}
                  onChange={(value) =>
                    !mutation.isPending && setDraft((current) => updateFile(current, selected.id, { content: value }))
                  }
                />
              </FormField>
            </>
          ) : (
            <EmptyState
              icon={FileCode2}
              title={t("playbooks.page.projects.files.empty", { defaultValue: "No files yet." })}
              description={isManual && issue === "empty" ? issueMessage : undefined}
            />
          )}
        </div>
      </div>
    </EntityEditorPage>
  );
}

export default function ProjectFilesPage() {
  const { id = "" } = useParams();
  const { t } = useTranslation();
  const navigate = useNavigate();

  const projectQuery = useQuery({
    queryKey: ["playbooks", "project", id],
    queryFn: () => getProject(id),
    enabled: id !== "",
  });
  const filesQuery = useQuery({
    queryKey: FILES_QUERY_KEY(id),
    queryFn: () => getProjectFiles(id),
    enabled: id !== "",
  });

  if (!id || !projectQuery.data || !filesQuery.data) {
    const pending = Boolean(id) && (projectQuery.isPending || filesQuery.isPending);
    const failed = projectQuery.isError || filesQuery.isError;
    const missing = [projectQuery.error, filesQuery.error].some((error) => error instanceof ApiError && error.status === 404);
    const back = () => void navigate(PLAYBOOK_TAB_ROUTES.projects);
    return (
      <EntityEditorPage scope="routed" width="standard"
        title={projectQuery.data?.name ?? t("playbooks.page.projects.files.title")}
        description={t("playbooks.page.projects.files.description")}
        breadcrumbs={[
          { key: "projects", label: t("playbooks.page.projects.title"), onClick: back },
          { key: "files", label: t("playbooks.page.projects.files.title") },
        ]}
        onBack={back} backLabel={t("back")} cancelLabel={t("cancel")}
      >
        {pending && !failed ? <output className="flex justify-center p-8">
          <Loader2 className="size-6 animate-spin" aria-label={t("loading")} />
        </output> : <div role="alert" className="flex flex-col items-start gap-4">
          <p>{t(failed && !missing ? "playbooks.page.projects.files.loadFailed" : "notFound.title")}</p>
          {failed && <Button variant="outline" onClick={() => { void projectQuery.refetch(); void filesQuery.refetch(); }}>{t("retry")}</Button>}
        </div>}
      </EntityEditorPage>
    );
  }

  return (
    <ProjectFilesEditor
      key={projectQuery.data.id}
      project={projectQuery.data}
      initialFiles={filesToDraft(filesQuery.data?.files ?? [])}
    />
  );
}
