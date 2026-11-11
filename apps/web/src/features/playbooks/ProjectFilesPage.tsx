import { useRef, useState } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowLeft, ChevronDown, ChevronRight, FileCode2, Folder, FolderPlus, Loader2, MoreHorizontal, Plus, Upload } from "lucide-react";

import { ApiError } from "@/shared/api/client";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { useUnsavedChangesGuard } from "@/shared/ui-shared/useUnsavedChangesGuard";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import TextEntryDialog from "@/shared/ui-shared/TextEntryDialog";
import { Sheet, SheetContent, SheetTitle, SheetTrigger } from "@/shared/ui-primitives/sheet";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/shared/ui-primitives/alert-dialog";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/shared/ui-primitives/dropdown-menu";
import { cn } from "@/shared/lib/utils";
import { IS_DEMO } from "@/demo/env";

import CodeEditor from "@/features/playbooks/CodeEditor";
import {
  addFile,
  buildFileTree,
  draftIssue,
  filesEqual,
  filesToDraft,
  nextFilePath,
  projectRevisionQueryKey,
  removeProjectPath,
  renameProjectPath,
  revisionChanges,
  updateFile,
  validProjectPath,
  type ProjectFileDraft,
  type ProjectFileTreeNode,
} from "@/features/playbooks/projectFilesModel";
import { changeProjectRevision, forkProject, getProject, getProjectRevision, listExecutionEnvironments, syncProject, updateProject } from "@/features/playbooks/api";
import type { Project, ProjectRevision } from "@/features/playbooks/api";
import { projectFormFromProject, projectIsSystemReadOnly, projectSettingsApplied, projectSource, validProjectForm, type ProjectForm } from "./projectFormModel";
import { FormField } from "./PlaybookFormFields";
import { ProjectAuthFields } from "./ProjectAuthFields";
import { useProjectAuthOptions } from "./useProjectAuthOptions";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";
import { projectSectionHref } from "./projectRoutes";

const FILES_QUERY_KEY = projectRevisionQueryKey;

/** One pending text entry over the file tree or the Git revision. */
type FilesEntryRequest =
  | { kind: "rename"; path: string; value: string }
  | { kind: "folder"; value: string }
  | { kind: "fork"; value: string };

/** Names the pending entry for the shared text dialog; one shape per case. */
function ProjectFilesEntryDialog({ request, onSubmit, onOpenChange }: Readonly<{
  request: FilesEntryRequest;
  onSubmit: (value: string) => string | undefined;
  onOpenChange: (open: boolean) => void;
}>) {
  const { t } = useTranslation();
  const copy = {
    rename: {
      title: t("playbooks.page.projects.workspace.rename", { defaultValue: "Rename" }),
      label: t("playbooks.page.projects.workspace.renamePrompt", { defaultValue: "New relative path" }),
      submitLabel: t("save"),
    },
    folder: {
      title: t("playbooks.page.projects.workspace.addFolder", { defaultValue: "Add folder" }),
      label: t("playbooks.page.projects.workspace.folderPrompt", { defaultValue: "Folder relative path" }),
      submitLabel: t("add"),
    },
    fork: {
      title: t("playbooks.page.projects.workspace.fork", { defaultValue: "Create editable copy" }),
      label: t("playbooks.page.projects.workspace.forkName", { defaultValue: "Copy name" }),
      submitLabel: t("create"),
    },
  }[request.kind];
  return (
    <TextEntryDialog
      title={copy.title}
      label={copy.label}
      defaultValue={request.value}
      submitLabel={copy.submitLabel}
      onSubmit={onSubmit}
      onOpenChange={onOpenChange}
    />
  );
}

function ProjectUploadButton({ onUpload }: Readonly<{ onUpload: (file: File) => void }>) {
  const { t } = useTranslation();
  const input = useRef<HTMLInputElement>(null);
  const label = t("playbooks.page.projects.workspace.upload", { defaultValue: "Upload binary file" });
  return <><Button type="button" variant="ghost" size="icon-sm" aria-label={label} onClick={() => input.current?.click()}><Upload aria-hidden="true" /></Button><Input ref={input} className="sr-only" type="file" tabIndex={-1} aria-label={label} onChange={(event) => { const file = event.target.files?.[0]; if (file) onUpload(file); event.target.value = ""; }} /></>;
}

function FileTreeNode({
  node,
  selectedFileId,
  onSelect,
  editable,
  onRename,
  onDelete,
  onAddToFolder,
}: Readonly<{
  node: ProjectFileTreeNode;
  selectedFileId: string | null;
  onSelect: (fileId: string) => void;
  editable: boolean;
  onRename: (path: string) => void;
  onDelete: (path: string) => void;
  onAddToFolder: (path: string) => void;
}>) {
  const [open, setOpen] = useState(true);
  const { t } = useTranslation();
  const actions = editable ? <DropdownMenu><DropdownMenuTrigger asChild><Button type="button" variant="ghost" size="icon-sm" aria-label={t("playbooks.page.projects.workspace.treeActions", { defaultValue: "Actions for {{path}}", path: node.path })}><MoreHorizontal aria-hidden="true" /></Button></DropdownMenuTrigger><DropdownMenuContent align="end">{node.kind === "folder" && <DropdownMenuItem onSelect={() => onAddToFolder(node.path)}>{t("playbooks.page.projects.workspace.addToFolder", { defaultValue: "Add file to {{path}}", path: node.path })}</DropdownMenuItem>}<DropdownMenuItem onSelect={() => onRename(node.path)}>{t("playbooks.page.projects.workspace.rename", { defaultValue: "Rename" })}</DropdownMenuItem><DropdownMenuItem variant="destructive" onSelect={() => onDelete(node.path)}>{t("delete")}</DropdownMenuItem></DropdownMenuContent></DropdownMenu> : null;
  if (node.kind === "folder") {
    return (
      <li>
        <div className="flex items-center"><button
          type="button"
          aria-expanded={open}
          className="flex min-w-0 flex-1 items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm text-muted-foreground hover:bg-accent/40"
          onClick={() => setOpen((prev) => !prev)}
        >
          {open ? (
            <ChevronDown className="size-3.5 shrink-0" />
          ) : (
            <ChevronRight className="size-3.5 shrink-0" />
          )}
          <Folder className="size-3.5 shrink-0" />
          <span className="truncate font-mono">{node.name}</span>
        </button>{actions}</div>
        {open && (
          <ul className="ml-3 border-l border-border pl-2">
            {node.children.map((child) => (
              <FileTreeNode
                key={child.fileId ?? `${child.kind}:${child.path}`}
                node={child}
                selectedFileId={selectedFileId}
                onSelect={onSelect}
                editable={editable}
                onRename={onRename}
                onDelete={onDelete}
                onAddToFolder={onAddToFolder}
              />
            ))}
          </ul>
        )}
      </li>
    );
  }
  return (
    <li>
      <div className="flex items-center"><button
        type="button"
        aria-current={node.fileId === selectedFileId ? "true" : undefined}
        className={cn(
          "flex min-w-0 flex-1 items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent/40",
          node.fileId === selectedFileId ? "bg-accent text-foreground" : "text-muted-foreground",
        )}
        onClick={() => node.fileId && onSelect(node.fileId)}
      >
        <FileCode2 className="size-3.5 shrink-0" />
        <span className="truncate font-mono">{node.name}</span>
      </button>{actions}</div>
    </li>
  );
}

function ProjectFilesEditor({
  project,
  initialFiles,
  revision,
}: Readonly<{
  project: Project;
  initialFiles: ProjectFileDraft[];
  revision: ProjectRevision | null;
}>) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const navigate = useNavigate();
  const location = useLocation();
  const queryClient = useQueryClient();
  const [baseline, setBaseline] = useState<ProjectFileDraft[]>(() => initialFiles);
  const [baseRevision, setBaseRevision] = useState(revision?.revision_id ?? null);
  const [draft, setDraft] = useState<ProjectFileDraft[]>(() => initialFiles);
  const [selectedId, setSelectedId] = useState<string | null>(initialFiles[0]?.id ?? null);
  const [openIds, setOpenIds] = useState<string[]>(() => initialFiles[0] ? [initialFiles[0].id] : []);
  const [treeOpen, setTreeOpen] = useState(false);
  const [folders, setFolders] = useState<string[]>([]);
  const [treeError, setTreeError] = useState("");
  const [deletePath, setDeletePath] = useState<string | null>(null);
  const [reloadConfirm, setReloadConfirm] = useState(false);
  const [entry, setEntry] = useState<FilesEntryRequest | null>(null);

  const [saveError, setSaveError] = useState<unknown>(null);
  const builtinReadOnly = projectIsSystemReadOnly(project);
  const [settings, setSettings] = useState<ProjectForm>(() => projectFormFromProject(project));
  const authOptions = useProjectAuthOptions();
  const [environmentId, setEnvironmentId] = useState("");
  const [actionError, setActionError] = useState("");
  const environments = useQuery({ queryKey: ["playbooks", "execution-environments"], queryFn: () => listExecutionEnvironments({ limit: 100, offset: 0 }), enabled: project.source.kind === "git" && can("READ", "PLAYBOOK_EXECUTION_ENVIRONMENT") });
  const settingsMutation = useMutation({ mutationFn: (submitted: ProjectForm) => updateProject(project.id, { name: submitted.name.trim(), description: submitted.description.trim() || null, source: projectSource(submitted) }), onSuccess: (updated, submitted) => { queryClient.setQueryData(["playbooks", "project", project.id], updated); void queryClient.invalidateQueries({ queryKey: ["playbooks", "projects"] }); if (!projectSettingsApplied(submitted, updated)) { setActionError(t("playbooks.page.projects.workspace.settingsNotApplied", { defaultValue: "Some project settings were not applied. Your changes are preserved." })); return; } setSettings((current) => current === submitted ? projectFormFromProject(updated) : current); if ((updated.current_revision_id ?? null) !== baseRevision) { setBaseRevision(updated.current_revision_id ?? null); setBaseline([]); setDraft([]); setSelectedId(null); setOpenIds([]); setFolders([]); setSaveError(null); } setActionError(""); }, onError: () => setActionError(t("playbooks.page.projects.workspace.settingsFailed", { defaultValue: "Could not update project settings." })) });
  /** Names the acquisition failure the operator can fix; the captured revision stays untouched. */
  const describeSyncFailure = (error: unknown) => {
    if (error instanceof ApiError) {
      if (error.reasonCode === "authentication_failed") return t("playbooks.page.projects.workspace.syncAuthFailed", { defaultValue: "Git rejected the credentials. Check the login, token or key." });
      if (error.reasonCode === "host_key_rejected") return t("playbooks.page.projects.workspace.syncHostKeyRejected", { defaultValue: "Git rejected the host key. Check the known host keys." });
      if (error.reasonCode === "reference_not_found") return t("playbooks.page.projects.workspace.syncReferenceNotFound", { defaultValue: "The branch or commit was not found in the repository." });
    }
    return t("playbooks.page.projects.workspace.syncFailed", { defaultValue: "Sync failed. Reload the project before retrying if it changed." });
  };
  const sync = useMutation({ mutationFn: () => syncProject(project.id, { base_revision_id: baseRevision, execution_environment_id: environmentId }), onSuccess: (result) => { const files = filesToDraft(result.entries); setBaseRevision(result.revision_id); setBaseline(files); setDraft(files); setSelectedId(files[0]?.id ?? null); setOpenIds(files[0] ? [files[0].id] : []); setActionError(""); queryClient.setQueryData(FILES_QUERY_KEY(project.id, result.revision_id), result); queryClient.setQueryData(["playbooks", "project", project.id], { ...project, current_revision_id: result.revision_id }); void queryClient.invalidateQueries({ queryKey: ["playbooks", "projects"] }); }, onError: (error) => setActionError(describeSyncFailure(error)) });
  const fork = useMutation({ mutationFn: (name: string) => forkProject(project.id, { revision_id: baseRevision!, name }), onSuccess: (result) => { void queryClient.invalidateQueries({ queryKey: ["playbooks", "projects"] }); void navigate(projectSectionHref(result.id, "files")); }, onError: () => setActionError(t("playbooks.page.projects.workspace.forkFailed", { defaultValue: "Could not fork this revision." })) });
  const save = useMutation({
    mutationFn: ({ submitted, base }: { submitted: ProjectFileDraft[]; base: string }) => changeProjectRevision(project.id, { base_revision_id: base, changes: revisionChanges(baseline, submitted) }),
    onSuccess: (result, { submitted }) => {
      const saved = filesToDraft(result.entries).map((file) => ({ ...file, id: submitted.find((item) => item.path === file.path)?.id ?? file.id }));
      setBaseRevision(result.revision_id);
      setBaseline(saved);
      setDraft((current) => filesEqual(current, submitted) ? saved : current);
      setFolders([]);
      setSaveError(null);
      queryClient.setQueryData(FILES_QUERY_KEY(project.id, result.revision_id), result);
      queryClient.setQueryData(["playbooks", "project", project.id], { ...project, current_revision_id: result.revision_id });
      void queryClient.invalidateQueries({ queryKey: ["playbooks", "projects"] });
    },
    onError: setSaveError,
  });
  const reload = useMutation({
    mutationFn: async () => {
      const latest = await getProject(project.id);
      return { latest, revision: latest.current_revision_id ? await getProjectRevision(project.id, latest.current_revision_id) : null };
    },
    onSuccess: ({ latest, revision: latestRevision }) => {
      const files = filesToDraft(latestRevision?.entries ?? []);
      setBaseRevision(latestRevision?.revision_id ?? null);
      setBaseline(files); setDraft(files); setFolders([]);
      setSelectedId(files[0]?.id ?? null); setOpenIds(files[0] ? [files[0].id] : []);
      setSaveError(null);
      setActionError("");
      setSettings(projectFormFromProject(latest));
      queryClient.setQueryData(["playbooks", "project", project.id], latest);
    },
    onError: setSaveError,
  });
  const isManual = project.source.kind === "editable";
  const editable = isManual && can("UPDATE", "PLAYBOOK_PROJECT") && can("CREATE", "PLAYBOOK_PROJECT_REVISION") && !save.isPending && !reload.isPending;
  const dirty = isManual && (!filesEqual(draft, baseline) || folders.length > 0);
  const { allowExit, confirmExit, discardDialog } = useUnsavedChangesGuard(dirty);
  const issue = draftIssue(draft);
  const selected = draft.find((file) => file.id === selectedId) ?? null;

  const back = () => {
    void navigate(PLAYBOOK_TAB_ROUTES.projects);
  };

  const selectFile = (id: string) => {
    setSelectedId(id);
    setOpenIds((current) => current.includes(id) ? current : [...current, id]);
    setTreeOpen(false);
  };

  const onAdd = (folder = "") => {
    const scoped = folder ? draft.filter((file) => file.path.startsWith(`${folder}/`)).map((file) => ({ ...file, path: file.path.slice(folder.length + 1) })) : draft;
    const path = `${folder ? `${folder}/` : ""}${nextFilePath(scoped)}`;
    const next = addFile(draft, path);
    setDraft(next);
    const created = next.at(-1);
    if (created) selectFile(created.id);
  };

  const onDelete = (path: string) => {
    const next = removeProjectPath(draft, path);
    setDraft(next);
    setFolders((current) => current.filter((folder) => folder !== path && !folder.startsWith(`${path}/`)));
    setOpenIds((current) => current.filter((fileId) => next.some((file) => file.id === fileId)));
    if (selectedId && !next.some((file) => file.id === selectedId)) {
      if (next[0]) selectFile(next[0].id);
      else setSelectedId(null);
    }
  };

  const pathTaken = (path: string) => draft.some((file) => file.path === path || path.startsWith(`${file.path}/`)) || folders.includes(path) || draft.some((file) => file.path.startsWith(`${path}/`)) || folders.some((folder) => folder.startsWith(`${path}/`));
  const pathConflict = t("playbooks.page.projects.workspace.pathConflict", { defaultValue: "That path is invalid or already exists." });
  const onRename = (path: string) => {
    setEntry({ kind: "rename", path, value: path });
  };
  const onAddFolder = () => {
    setEntry({ kind: "folder", value: "" });
  };
  // Applies the pending entry. Returning a message keeps the dialog open with
  // the typed draft and that error; renaming to the current path just closes it.
  const submitEntry = (value: string): string | undefined => {
    if (!entry) return undefined;
    if (entry.kind === "rename") {
      const path = entry.path;
      if (value === path) return undefined;
      if (!validProjectPath(value) || value.startsWith(`${path}/`) || pathTaken(value)) return pathConflict;
      const renamed = renameProjectPath(draft, path, value);
      if (renamed === draft && draft.some((file) => file.path === path || file.path.startsWith(`${path}/`))) return pathConflict;
      setDraft(renamed);
      setFolders((current) => current.map((folder) => folder === path || folder.startsWith(`${path}/`) ? value + folder.slice(path.length) : folder));
      setTreeError("");
      return undefined;
    }
    if (entry.kind === "folder") {
      if (!validProjectPath(value) || pathTaken(value)) return pathConflict;
      setFolders((current) => [...current, value]);
      setTreeError("");
      return undefined;
    }
    if (fork.isPending) return undefined;
    fork.mutate(value.trim());
    return undefined;
  };
  const tree = buildFileTree(draft, folders);
  const treeNodes = tree.map((node) => <FileTreeNode key={node.fileId ?? node.path} node={node} selectedFileId={selectedId} onSelect={selectFile} editable={editable} onRename={onRename} onDelete={setDeletePath} onAddToFolder={onAdd} />);
  const upload = (file: File) => {
    if (!validProjectPath(file.name) || pathTaken(file.name)) {
      setTreeError(pathConflict);
      return;
    }
    const reader = new FileReader();
    reader.onerror = () => setTreeError(t("playbooks.page.projects.workspace.uploadFailed", { defaultValue: "Could not read the selected file." }));
    reader.onload = () => {
      if (!(reader.result instanceof ArrayBuffer)) return;
      const next = addFile(draft, file.name);
      const created = next.at(-1);
      if (!created) return;
      setDraft((current) => {
        if (current.some((item) => item.path === file.name)) return current;
        return [...current, { ...created, bytes: new Uint8Array(reader.result as ArrayBuffer) }];
      });
      selectFile(created.id);
      setTreeError("");
    };
    reader.readAsArrayBuffer(file);
  };
  const download = (file: ProjectFileDraft) => {
    const bytes = file.bytes ? new Uint8Array(file.bytes) : new TextEncoder().encode(file.content);
    const url = URL.createObjectURL(new Blob([bytes.buffer], { type: "application/octet-stream" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = file.path.split("/").at(-1) || "download";
    link.click();
    URL.revokeObjectURL(url);
  };
  const closeTab = (id: string) => {
    const remaining = openIds.filter((item) => item !== id);
    setOpenIds(remaining);
    if (selectedId === id) setSelectedId(remaining.at(-1) ?? null);
  };

  const issueMessage = (() => {
    if (issue === "blankPath") {
      return t("playbooks.page.projects.files.pathRequired", { defaultValue: "File path is required." });
    }
    if (issue === "invalidPath") return t("playbooks.page.projects.files.invalidPath", { defaultValue: "Use a safe relative path without empty, dot or parent segments." });
    if (issue === "duplicatePath") {
      return t("playbooks.page.projects.files.duplicatePath", {
        defaultValue: "Two files share this path.",
      });
    }
    return "";
  })();

  const treeToolbar = editable ? <div className="flex items-center gap-1 border-b border-border pb-2">
    <Button type="button" variant="ghost" size="icon-sm" aria-label={t("playbooks.page.projects.files.add", { defaultValue: "Add file" })} onClick={() => onAdd()}><Plus aria-hidden="true" /></Button>
    <Button type="button" variant="ghost" size="icon-sm" aria-label={t("playbooks.page.projects.workspace.addFolder", { defaultValue: "Add folder" })} onClick={onAddFolder}><FolderPlus aria-hidden="true" /></Button>
    <ProjectUploadButton onUpload={upload} />
  </div> : null;
  const guardedBack = () => { if (dirty && !allowExit.current) confirmExit(back); else back(); };
  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col gap-2 overflow-hidden">
      <PageChrome title={project.name} description={`${project.source.kind === "git" ? project.source.url : project.source.kind} · ${baseRevision || t("playbooks.page.projects.noRevision", { defaultValue: "No revision" })}`} actions={<Button type="button" variant="ghost" size="sm" onClick={guardedBack}><ArrowLeft aria-hidden="true" />{t("back")}</Button>} />
      {discardDialog}
      {entry ? <ProjectFilesEntryDialog request={entry} onSubmit={submitEntry} onOpenChange={(open) => { if (!open) setEntry(null); }} /> : null}
      <AlertDialog open={deletePath !== null} onOpenChange={(open) => { if (!open) setDeletePath(null); }}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{t("playbooks.page.projects.workspace.deleteTitle", { defaultValue: "Delete project files?" })}</AlertDialogTitle><AlertDialogDescription>{t("playbooks.page.projects.workspace.confirmDelete", { defaultValue: "Delete {{path}} and everything inside it?", path: deletePath ?? "" })}</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>{t("cancel")}</AlertDialogCancel><AlertDialogAction onClick={() => { if (deletePath) onDelete(deletePath); }}>{t("delete")}</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
      <AlertDialog open={reloadConfirm} onOpenChange={setReloadConfirm}><AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{t("playbooks.page.projects.workspace.reloadDiscard", { defaultValue: "Discard and reload" })}</AlertDialogTitle><AlertDialogDescription>{t("playbooks.page.projects.workspace.discardReload", { defaultValue: "Discard your draft and reload the current revision?" })}</AlertDialogDescription></AlertDialogHeader><AlertDialogFooter><AlertDialogCancel>{t("cancel")}</AlertDialogCancel><AlertDialogAction onClick={() => reload.mutate()}>{t("playbooks.page.projects.workspace.reloadDiscard", { defaultValue: "Discard and reload" })}</AlertDialogAction></AlertDialogFooter></AlertDialogContent></AlertDialog>
      <SectionLinks label={t("playbooks.page.projects.title")} activeValue={location.pathname.split("/").at(-1) || "files"} items={(["files", "jobs", "source", "settings"] as const).map((section) => ({ value: section, label: t(`playbooks.page.projects.workspace.${section}`, { defaultValue: section[0]!.toUpperCase() + section.slice(1) }), to: projectSectionHref(project.id, section) }))} />
      {actionError && <div role="alert" className="flex items-center gap-2 text-destructive">{actionError}<Button variant="outline" onClick={() => setReloadConfirm(true)}>{t("playbooks.page.projects.workspace.reloadDiscard", { defaultValue: "Discard and reload" })}</Button></div>}
      {location.pathname.endsWith("/jobs") ? <div className="flex flex-col gap-3"><Button asChild variant="outline"><Link to={`/playbooks/jobs/new?project_id=${encodeURIComponent(project.id)}`}>{t("playbooks.page.projects.workspace.createJob", { defaultValue: "Create job" })}</Link></Button></div> : location.pathname.endsWith("/source") ? <div className="flex flex-col items-start gap-3">
        <p>{project.source.kind === "git" ? `${project.source.url} · ${project.source.reference}` : t("playbooks.page.projects.manualSource")}</p><p>{baseRevision ?? t("playbooks.page.projects.noRevision", { defaultValue: "No revision" })}</p>
        {IS_DEMO && <p role="note">{t("playbooks.page.projects.workspace.syntheticSourceNotice", { defaultValue: "Demo only: sync and fork use synthetic content and do not contact Git." })}</p>}
        {project.source.kind === "git" && can("UPDATE", "PLAYBOOK_PROJECT") && can("EXECUTE_NATIVE", "PLAYBOOK_PROJECT") && can("READ", "PLAYBOOK_EXECUTION_ENVIRONMENT") && (project.source.auth.kind === "anonymous" || can("EXECUTE", "SECRET_VARIABLE") && can("DELEGATE_CREDENTIALS", "SECRET_VARIABLE")) && <><FormSelect label={t("playbooks.page.fields.executionEnvironment")} value={environmentId} onValueChange={setEnvironmentId} options={(environments.data?.items ?? []).filter((env) => env.enabled).map((env) => ({ value: env.id, label: env.name }))} /><Button disabled={!environmentId || sync.isPending} onClick={() => sync.mutate()}>{t("playbooks.page.projects.workspace.sync", { defaultValue: "Sync" })}</Button></>}
        {baseRevision && can("READ", "PLAYBOOK_PROJECT") && can("CREATE", "PLAYBOOK_PROJECT") && can("READ", "PLAYBOOK_PROJECT_REVISION") && <Button disabled={fork.isPending} onClick={() => setEntry({ kind: "fork", value: "" })}>{t("playbooks.page.projects.workspace.fork", { defaultValue: "Fork revision" })}</Button>}
      </div> : location.pathname.endsWith("/settings") ? <fieldset disabled={builtinReadOnly || !can("UPDATE", "PLAYBOOK_PROJECT")} className="grid max-w-xl gap-3">
        {builtinReadOnly && <p className="text-xs text-muted-foreground">{t("playbooks.page.projects.workspace.builtinReadOnly", { defaultValue: "This is the built-in catalog project. Its source, files and job definitions are product-owned; configure targets and the execution environment on a job instead." })}</p>}
        <FormField label={t("name")}><Input value={settings.name} onChange={(e) => setSettings((p) => ({ ...p, name: e.target.value }))} /></FormField>
        <FormField label={t("playbooks.page.fields.description")}><Input value={settings.description} onChange={(e) => setSettings((p) => ({ ...p, description: e.target.value }))} /></FormField>
        {settings.sourceKind === "git" && <>
          <FormField label={t("playbooks.page.projects.scmUrl")}><Input value={settings.url} onChange={(e) => setSettings((p) => ({ ...p, url: e.target.value }))} /></FormField>
          <FormField label={t("playbooks.page.projects.ref")}><Input value={settings.reference} onChange={(e) => setSettings((p) => ({ ...p, reference: e.target.value }))} /></FormField>
          <ProjectAuthFields form={settings} onChange={setSettings} options={authOptions} disabled={!can("UPDATE", "PLAYBOOK_PROJECT")} />
        </>}
        {!builtinReadOnly && can("UPDATE", "PLAYBOOK_PROJECT") && <Button disabled={settingsMutation.isPending || !validProjectForm(settings)} onClick={() => settingsMutation.mutate(settings)}>{t("save")}</Button>}
      </fieldset> : <>
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <Sheet open={treeOpen} onOpenChange={setTreeOpen}>
          <SheetTrigger asChild><Button variant="outline" size="sm" className="md:hidden">{t("playbooks.page.projects.workspace.browseFiles", { defaultValue: "Browse files" })}</Button></SheetTrigger>
          <SheetContent side="left" className="flex flex-col gap-2 overflow-hidden"><SheetTitle>{t("playbooks.page.projects.workspace.files", { defaultValue: "Files" })}</SheetTitle>{treeToolbar}{treeError && <p role="alert" className="text-sm text-destructive">{treeError}</p>}<ul className="min-h-0 flex-1 overflow-y-auto">{treeNodes}</ul></SheetContent>
        </Sheet>
        {isManual && can("UPDATE", "PLAYBOOK_PROJECT") && can("CREATE", "PLAYBOOK_PROJECT_REVISION") ? <Button type="button" size="sm" disabled={!editable || !dirty || !!issue || !baseRevision || revisionChanges(baseline, draft).length === 0 || saveError instanceof ApiError && saveError.status === 409} onClick={() => { if (baseRevision) save.mutate({ submitted: draft, base: baseRevision }); }}>{t("save")}</Button> : <span role="status">{t("playbooks.page.projects.workspace.readOnly", { defaultValue: "Read only" })}</span>}
      </div>
      {saveError != null && <div role="alert" className="flex flex-wrap items-center gap-2 text-destructive"><span>{saveError instanceof ApiError && saveError.status === 409 ? t("playbooks.page.projects.workspace.conflict", { defaultValue: "The project changed on the server. Your draft is preserved." }) : t("playbooks.page.projects.workspace.saveFailed", { defaultValue: "Could not save. Your draft is preserved." })}</span><Button type="button" variant="outline" disabled={reload.isPending} onClick={() => setReloadConfirm(true)}>{t("playbooks.page.projects.workspace.reloadDiscard", { defaultValue: "Discard and reload" })}</Button></div>}
      {treeError && !treeOpen && <p role="alert" className="text-sm text-destructive">{treeError}</p>}
      <div className="flex min-h-0 min-w-0 flex-1 gap-2 overflow-hidden">
        <aside className="hidden min-h-0 w-60 shrink-0 flex-col gap-2 border-r border-border pr-2 md:flex" aria-label={t("playbooks.page.projects.workspace.files", { defaultValue: "Files" })}>
          {treeToolbar}
          <ul className="min-h-0 flex-1 overflow-y-auto">{treeNodes}</ul>
        </aside>
        <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-1">
          <div className="flex min-w-0 gap-1 overflow-x-auto" aria-label={t("playbooks.page.projects.workspace.openFiles", { defaultValue: "Open files" })}>{openIds.map((id) => { const file = draft.find((item) => item.id === id); return file ? <div key={id} className="flex shrink-0 items-center"><Button type="button" size="sm" variant={id === selectedId ? "secondary" : "ghost"} onClick={() => selectFile(id)}>{file.path.split("/").at(-1)}</Button><Button type="button" size="icon-sm" variant="ghost" aria-label={t("playbooks.page.projects.workspace.closeFile", { defaultValue: "Close {{path}}", path: file.path })} onClick={() => closeTab(id)}>×</Button></div> : null; })}</div>
          {selected ? <>
            <div className="flex min-w-0 items-center gap-2 border-b border-border pb-1">
              <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground" title={selected.path}>{selected.path}</span>
              <DropdownMenu><DropdownMenuTrigger asChild><Button type="button" size="icon-sm" variant="ghost" aria-label={t("playbooks.page.projects.workspace.fileActions", { defaultValue: "File actions" })}><MoreHorizontal aria-hidden="true" /></Button></DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  {selected.symlinkTarget === undefined && <DropdownMenuItem onSelect={() => download(selected)}>{t("playbooks.page.projects.workspace.download", { defaultValue: "Download file" })}</DropdownMenuItem>}
                  {editable && <><DropdownMenuItem onSelect={() => onRename(selected.path)}>{t("playbooks.page.projects.workspace.rename", { defaultValue: "Rename" })}</DropdownMenuItem>{selected.symlinkTarget === undefined && <DropdownMenuCheckboxItem checked={selected.executable} onCheckedChange={(checked) => setDraft((current) => updateFile(current, selected.id, { executable: checked }))}>{t("playbooks.page.projects.workspace.executableFile", { defaultValue: "Executable file" })}</DropdownMenuCheckboxItem>}<DropdownMenuItem variant="destructive" onSelect={() => setDeletePath(selected.path)}>{t("delete")}</DropdownMenuItem></>}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            {issueMessage && <p role="alert" className="text-xs text-destructive">{issueMessage}</p>}
            {selected.symlinkTarget !== undefined ? <p>{t("playbooks.page.projects.workspace.symlink", { defaultValue: "Symbolic link" })}: {selected.symlinkTarget}</p> : selected.bytes ? <p>{t("playbooks.page.projects.workspace.binaryFile", { defaultValue: "Binary file ({{size}} bytes)", size: selected.bytes.length })}</p> : <div className="min-h-0 flex-1"><CodeEditor documentId={`${project.id}:${selected.id}`} fileName={selected.path} value={selected.content} readOnly={!editable} height="100%" minHeight="0" aria-label={t("playbooks.page.projects.files.content", { defaultValue: "Content" })} onChange={(value) => setDraft((current) => updateFile(current, selected.id, { content: value }))} /></div>}
          </> : <EmptyState icon={FileCode2} title={t("playbooks.page.projects.files.empty", { defaultValue: "No files yet." })} />}
        </div>
      </div>
      </>}
    </div>
  );
}

export default function ProjectFilesPage() {
  const { projectId: id = "" } = useParams();
  const { t } = useTranslation();
  const { can } = usePermissions();
  const navigate = useNavigate();

  const projectQuery = useQuery({
    queryKey: ["playbooks", "project", id],
    queryFn: () => getProject(id),
    enabled: id !== "" && can("READ", "PLAYBOOK_PROJECT"),
  });
  const revisionId = projectQuery.data?.current_revision_id;
  const filesQuery = useQuery({
    queryKey: FILES_QUERY_KEY(id, revisionId ?? ""),
    queryFn: () => getProjectRevision(id, revisionId!),
    enabled: !!id && !!revisionId && can("READ", "PLAYBOOK_PROJECT") && can("READ", "PLAYBOOK_PROJECT_REVISION"),
  });

  if (!id || !projectQuery.data || (!!revisionId && !filesQuery.data)) {
    const pending = Boolean(id) && (projectQuery.fetchStatus === "fetching" || filesQuery.fetchStatus === "fetching");
    const failed = projectQuery.isError || filesQuery.isError;
    const missing = [projectQuery.error, filesQuery.error].some((error) => error instanceof ApiError && error.status === 404);
    const back = () => void navigate(PLAYBOOK_TAB_ROUTES.projects);
    return (
      <EntityEditorPage scope="routed"
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
      initialFiles={filesToDraft(filesQuery.data?.entries ?? [])}
      revision={filesQuery.data ?? null}
    />
  );
}
