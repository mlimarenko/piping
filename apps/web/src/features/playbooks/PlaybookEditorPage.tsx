import { useMemo, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, GitBranch, Loader2, ShieldCheck } from "lucide-react";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import { useApiMutation } from "@/shared/api/hooks";
import { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { cn } from "@/shared/lib/utils";
import { FormField } from "./PlaybookFormFields";
import CodeEditor from "@/features/playbooks/CodeEditor";
import VersionHistory from "@/features/playbooks/VersionHistory";
import VarsSchemaEditor from "@/features/playbooks/VarsSchemaEditor";
import PlaybookSecretsEditor from "@/features/playbooks/PlaybookSecretsEditor";
import { serializeVarsSchema } from "@/features/playbooks/varsSchema";
import {
  secretEnvUnchanged,
  serializeSecretEnv,
  writeSecretEnv,
} from "@/features/playbooks/secretsPolicy";
import {
  AUTOMATION_SECRETS_QUERY_KEY,
  listAutomationSecrets,
  type AutomationSecret,
} from "@/features/automation-secrets/api";
import {
  createPlaybookCopyDraft,
  formFromPlaybookItem,
  NO_TARGET_SET,
  playbookEditorSlugify,
  type PlaybookEditorFormState,
  type PlaybookEditorTarget,
} from "@/features/playbooks/playbookEditorModel";
import {
  createPlaybook,
  createPlaybookVersion,
  getPlaybook,
  listPlaybookTargetSets,
  updatePlaybook,
  type CreatePlaybookRequest,
  type PlaybookCatalogItem,
  type PlaybookConnection,
  type PlaybookRisk,
  type PlaybookStatus,
  type PlaybookTargetSet,
  type PlaybookVersion,
  type UpdatePlaybookRequest,
} from "@/features/playbooks/api";
import { playbookReturnHref } from "./playbookDetailRoutes";
import {
  PLAYBOOK_EDITOR_SOURCE_PARAM,
  PLAYBOOK_EDITOR_TAB_PARAM,
  playbookEditorCopyHref,
} from "@/features/playbooks/playbookEditorRoutes";

const RISKS: PlaybookRisk[] = ["low", "medium", "high", "critical"];
const STATUSES: PlaybookStatus[] = ["draft", "active", "archived"];
const CONNECTIONS: PlaybookConnection[] = ["ssh", "winrm", "local"];

function persistedPlaybook(target: PlaybookEditorTarget | null): PlaybookCatalogItem | null {
  if (target?.mode === "edit" || target?.mode === "view") return target.item;
  return null;
}

function sourcePlaybook(
  target: PlaybookEditorTarget | null,
  persistedItem: PlaybookCatalogItem | null,
): PlaybookCatalogItem | null {
  if (target?.mode === "copyDraft") return target.source;
  return persistedItem;
}

function formForTarget(target: PlaybookEditorTarget | null): PlaybookEditorFormState {
  if (target?.mode === "copyDraft") return target.draft;
  if (target?.mode === "edit" || target?.mode === "view") {
    return formFromPlaybookItem(target.item);
  }
  return formFromPlaybookItem(null);
}

function initialTabForTarget(target: PlaybookEditorTarget | null): "editor" | "history" {
  if (target?.mode === "edit" || target?.mode === "view") {
    return target.initialTab ?? "editor";
  }
  return "editor";
}

function createRequest(form: PlaybookEditorFormState): CreatePlaybookRequest {
  const hasSecretDeclarations = Object.keys(serializeSecretEnv(form.secretsEnv)).length > 0;
  return {
    name: form.name.trim(),
    slug: form.slug.trim() || null,
    description: form.description.trim() || null,
    category: form.category.trim() || null,
    risk: form.risk,
    status: form.status,
    runtime: "ansible",
    language: "yaml",
    connection_default: form.connectionDefault,
    vars_schema: serializeVarsSchema(form.varsSchema),
    content: form.content,
    timeout_seconds: Number.parseInt(form.timeoutSeconds, 10) || null,
    ...(hasSecretDeclarations
      ? { token_policy: writeSecretEnv(undefined, form.secretsEnv) }
      : {}),
  };
}

function updateRequest(form: PlaybookEditorFormState): UpdatePlaybookRequest {
  return {
    name: form.name.trim(),
    description: form.description.trim() || null,
    category: form.category.trim() || null,
    risk: form.risk,
    status: form.status,
    connection_default: form.connectionDefault,
    vars_schema: serializeVarsSchema(form.varsSchema),
    default_target_set_id:
      form.defaultTargetSetId === NO_TARGET_SET ? null : form.defaultTargetSetId,
  };
}

function updatedVersion(
  form: PlaybookEditorFormState,
  persistedItem: PlaybookCatalogItem,
): { content: string; tokenPolicy: unknown } | null {
  const currentTokenPolicy = persistedItem.current_version?.token_policy;
  const contentChanged =
    persistedItem.current_version?.content !== undefined &&
    form.content !== persistedItem.current_version.content;
  const secretsChanged = !secretEnvUnchanged(currentTokenPolicy, form.secretsEnv);
  if (!contentChanged && !secretsChanged) return null;
  return {
    content: form.content,
    tokenPolicy: writeSecretEnv(currentTokenPolicy, form.secretsEnv),
  };
}

// i18n key for the read-only managed-source banner, resolved by source kind.
function managedBannerKey(isSystem: boolean, isOfficial: boolean): string {
  if (isSystem) return "playbooks.page.editor.systemBanner";
  if (isOfficial) return "playbooks.page.editor.officialBanner";
  return "playbooks.page.editor.managedBanner";
}

function editorTitle(
  target: PlaybookEditorTarget | null,
  isPersisted: boolean,
  readOnly: boolean,
  name: string,
  t: ReturnType<typeof useTranslation>["t"],
): string {
  if (target?.mode === "copyDraft") {
    return t("playbooks.page.editor.copyDraftTitle", {
      name: target.source.playbook.name,
    });
  }
  if (!isPersisted) return t("playbooks.page.editor.createTitle");
  return readOnly
    ? t("playbooks.page.editor.viewTitle", { name })
    : t("playbooks.page.editor.editTitle", { name });
}

export interface PlaybookEditorPageProps {
  /** Explicit editor target. Copy drafts are unsaved until the user presses Create. */
  target: PlaybookEditorTarget | null;
  targetSets: PlaybookTargetSet[];
  /** Invalidate the playbook caches after a successful mutation. */
  onSaved: () => void;
  /** Leaves the editor (Back / Cancel / after a successful save). */
  onBack: () => void;
  /** Managed/system playbooks are read-only; this opens an unsaved editable draft. */
  onUseAsTemplate?: (item: PlaybookCatalogItem) => void;
}

/**
 * Create + edit full-page editor for in-app Ansible playbooks, built on the
 * canonical {@link EntityEditorPage} chrome so the YAML CodeEditor gets a real
 * scroll region under a sticky header and footer. Runtime is pinned to
 * `ansible`; the author picks the default per-host connection (ssh/winrm) and
 * declares the typed launch `vars_schema`. Edit mode also exposes immutable
 * version history (view / diff / restore). Git-mirrored playbooks
 * (`source_kind !== 'in_app'`) render fully read-only with a managed-source
 * banner and no save/restore affordances.
 */
function PlaybookEditorShell({
  target,
  targetSets,
  onSaved,
  onBack,
  onUseAsTemplate,
}: Readonly<PlaybookEditorPageProps>) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const persistedItem = persistedPlaybook(target);
  const sourceItem = sourcePlaybook(target, persistedItem);
  const isPersisted = persistedItem !== null;
  const isCreateLike = target?.mode === "create" || target?.mode === "copyDraft";
  const isManaged = target?.mode === "view" && sourceItem?.playbook.source_kind !== "in_app";
  const isSystem = sourceItem?.playbook.source_kind === "system";
  const isOfficial = sourceItem?.playbook.source_kind === "official";
  const readOnly = target?.mode === "view";

  const [form, setForm] = useState<PlaybookEditorFormState>(() => formFromPlaybookItem(null));
  const [tab, setTab] = useState<"editor" | "history">("editor");

  // Named secrets feed the editor's secret picker. Fetched while the editor is
  // mounted; a caller without READ×SECRET_VARIABLE simply gets an empty list
  // (403 is not retried), which degrades the picker gracefully.
  const secretsQuery = useQuery({
    queryKey: AUTOMATION_SECRETS_QUERY_KEY,
    queryFn: listAutomationSecrets,
    enabled: true,
  });
  const secrets = useMemo<AutomationSecret[]>(
    () => secretsQuery.data ?? [],
    [secretsQuery.data],
  );

  // Reset the form whenever the editor mounts for a (possibly different) item.
  // Adjusting state during render keeps the first paint consistent, instead of
  // briefly showing the previous item's form.
  const [syncedTarget, setSyncedTarget] = useState<PlaybookEditorTarget | null>(null);
  if (target !== syncedTarget) {
    setSyncedTarget(target);
    setForm(formForTarget(target));
    setTab(initialTabForTarget(target));
  }

  const update = <K extends keyof PlaybookEditorFormState>(key: K, value: PlaybookEditorFormState[K]) => {
    if (!locked) setForm((prev) => ({ ...prev, [key]: value }));
  };

  const handleSaved = (message: string) => {
    void Promise.all([
      queryClient.invalidateQueries({ queryKey: ["playbooks", "catalog"] }),
      persistedItem
        ? queryClient.invalidateQueries({
            queryKey: ["playbooks", "versions", persistedItem.playbook.id],
          })
        : Promise.resolve(),
    ]);
    onSaved();
    notify("success", message);
  };

  const createMutation = useApiMutation({
    mutationFn: (body: CreatePlaybookRequest) => createPlaybook(body),
    onSuccess: (created) => {
      handleSaved(
        t("playbooks.page.toasts.playbookCreated", { name: created.playbook.name }),
      );
      onBack();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  // Edit save: PATCH metadata, then (only if the content OR the secret
  // declaration changed) POST a new immutable version. token_policy is
  // versioned — not part of UpdatePlaybookRequest — so any secret edit rides a
  // new version carrying the merged token_policy. The metadata patch always
  // runs so name/risk/status/default-target-set edits land regardless.
  const editMutation = useApiMutation({
    mutationFn: async (vars: {
      id: string;
      meta: UpdatePlaybookRequest;
      newVersion: { content: string; tokenPolicy: unknown } | null;
      changelog: string;
    }) => {
      await updatePlaybook(vars.id, vars.meta);
      if (vars.newVersion) {
        await createPlaybookVersion(vars.id, {
          content: vars.newVersion.content,
          token_policy: vars.newVersion.tokenPolicy,
          changelog: vars.changelog.trim() || null,
        });
      }
    },
    onSuccess: () => {
      handleSaved(t("playbooks.page.toasts.playbookUpdated"));
      onBack();
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const restoreMutation = useApiMutation({
    mutationFn: (vars: { id: string; version: PlaybookVersion }) =>
      createPlaybookVersion(vars.id, {
        content: vars.version.content,
        // Restore the version's token_policy too, so a restore never silently
        // drops the secret declarations that version carried.
        token_policy: vars.version.token_policy,
        changelog: t("playbooks.page.versions.restoreChangelog", {
          number: vars.version.version,
        }),
      }),
    onSuccess: (_created, vars) => {
      handleSaved(
        t("playbooks.page.toasts.versionRestored", { number: vars.version.version }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const saving = createMutation.isPending || editMutation.isPending;
  const locked = saving || restoreMutation.isPending;

  const submit = () => {
    if (readOnly || locked) return;
    if (!form.name.trim() || !form.content.trim()) {
      notify("error", t("playbooks.page.errors.playbookRequired"));
      return;
    }
    if (isCreateLike || !persistedItem) {
      createMutation.mutate(createRequest(form));
      return;
    }
    editMutation.mutate({
      id: persistedItem.playbook.id,
      meta: updateRequest(form),
      newVersion: updatedVersion(form, persistedItem),
      changelog: form.changelog,
    });
  };

  const targetSetOptions = useMemo(() => targetSets, [targetSets]);

  const title = editorTitle(target, isPersisted, readOnly, form.name, t);

  const footer = (
    <div className="flex w-full items-center justify-end gap-2">
      <Button data-editor-cancel variant="outline" disabled={locked} onClick={onBack}>
        {readOnly ? t("close") : t("cancel")}
      </Button>
      {readOnly && persistedItem && onUseAsTemplate && (
        <Button
          className="gap-1.5"
          onClick={() => onUseAsTemplate(persistedItem)}
        >
          <Copy className="h-3.5 w-3.5" />
          {t("playbooks.page.editor.useAsTemplate")}
        </Button>
      )}
      {!readOnly && (
        <Button onClick={submit} disabled={locked}>
          {isPersisted ? t("playbooks.page.editor.save") : t("create")}
        </Button>
      )}
    </div>
  );

  return (
    <EntityEditorPage scope="routed"
      title={title}
      description={readOnly ? undefined : t("playbooks.page.editor.description")}
      onBack={() => { if (!locked) onBack(); }}
      backLabel={t("back")}
      footer={footer}
      dirty={!readOnly && JSON.stringify(form) !== JSON.stringify(formForTarget(target))}
      dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}
      width="standard"
    >
      <fieldset disabled={locked} inert={locked} className="min-w-0">
      <TooltipProvider delayDuration={200}>
        {isManaged && (
          <div className="mb-3 flex items-center gap-2 rounded-md border border-status-warning/30 bg-status-warning/10 px-3 py-2 text-xs text-status-warning">
            {isSystem || isOfficial ? (
              <ShieldCheck className="h-3.5 w-3.5 shrink-0" />
            ) : (
              <GitBranch className="h-3.5 w-3.5 shrink-0" />
            )}
            {t(managedBannerKey(isSystem, isOfficial))}
          </div>
        )}

        {isPersisted ? (
          <Tabs value={tab} onValueChange={(value) => setTab(value as typeof tab)}>
            <SectionTabs
              items={[
                { value: "editor", label: t("playbooks.page.editor.tabEditor") },
                { value: "history", label: t("playbooks.page.editor.tabHistory") },
              ] satisfies Array<{ value: typeof tab; label: string }>}
            />
            <TabsContent value="editor" className="mt-3">
              <EditorForm
                form={form}
                update={update}
                readOnly={readOnly || locked}
                isEdit={isPersisted}
                targetSets={targetSetOptions}
                secrets={secrets}
                secretsLoading={secretsQuery.isPending}
              />
            </TabsContent>
            <TabsContent value="history" className="mt-3">
              <VersionHistory
                playbookId={persistedItem.playbook.id}
                currentVersionId={persistedItem.playbook.current_version_id}
                readOnly={readOnly || locked}
                restoring={restoreMutation.isPending}
                onRestore={(version) =>
                  restoreMutation.mutate({ id: persistedItem.playbook.id, version })
                }
              />
            </TabsContent>
          </Tabs>
        ) : (
          <EditorForm
            form={form}
            update={update}
            readOnly={readOnly || locked}
            isEdit={isPersisted}
            targetSets={targetSetOptions}
            secrets={secrets}
            secretsLoading={secretsQuery.isPending}
          />
        )}
      </TooltipProvider>
      </fieldset>
    </EntityEditorPage>
  );
}

function EditorForm({
  form,
  update,
  readOnly,
  isEdit,
  targetSets,
  secrets,
  secretsLoading,
}: {
  form: PlaybookEditorFormState;
  update: <K extends keyof PlaybookEditorFormState>(
    key: K,
    value: PlaybookEditorFormState[K],
  ) => void;
  readOnly: boolean;
  isEdit: boolean;
  targetSets: PlaybookTargetSet[];
  secrets: AutomationSecret[];
  secretsLoading: boolean;
}) {
  const { t } = useTranslation();
  const fieldCls = "h-9 text-sm";

  const onNameChange = (name: string) => {
    update("name", name);
    // Auto-derive the slug from the name on create until the user edits it.
    if (!isEdit && !form.slug) {
      update("slug", playbookEditorSlugify(name));
    }
  };

  return (
    <div className="space-y-3">
      <div className="grid gap-3 md:grid-cols-2">
        <FormField label={t("name")} required>
          <Input
            className={fieldCls}
            value={form.name}
            disabled={readOnly}
            onChange={(e) => onNameChange(e.target.value)}
          />
        </FormField>
        <FormField label="Slug">
          <Input
            className={cn(fieldCls, "font-mono")}
            value={form.slug}
            // Slug is immutable after creation.
            disabled={readOnly || isEdit}
            onChange={(e) => update("slug", e.target.value)}
          />
        </FormField>
        <FormField label={t("description")} className="md:col-span-2">
          <Input
            className={fieldCls}
            value={form.description}
            disabled={readOnly}
            onChange={(e) => update("description", e.target.value)}
          />
        </FormField>
        <FormField label={t("playbooks.page.fields.category")}>
          <Input
            className={fieldCls}
            value={form.category}
            disabled={readOnly}
            onChange={(e) => update("category", e.target.value)}
          />
        </FormField>
        <FormField label={t("playbooks.page.fields.timeout")}>
          <Input
            className={fieldCls}
            inputMode="numeric"
            value={form.timeoutSeconds}
            disabled={readOnly}
            onChange={(e) => update("timeoutSeconds", e.target.value)}
          />
        </FormField>
        <FormField label={t("playbooks.page.fields.risk")}>
          <FormSelect
            value={form.risk}
            disabled={readOnly}
            onValueChange={(value) => update("risk", value)}
            triggerClassName={fieldCls}
            options={RISKS.map((risk) => ({
              value: risk,
              label: t(`playbooks.page.risk.${risk}`),
            }))}
          />
        </FormField>
        <FormField label={t("status")}>
          <FormSelect
            value={form.status}
            disabled={readOnly}
            onValueChange={(value) => update("status", value)}
            triggerClassName={fieldCls}
            options={STATUSES.map((status) => ({
              value: status,
              label: t(`playbooks.page.status.${status}`),
            }))}
          />
        </FormField>
        <FormField
          label={t("playbooks.page.fields.runtime")}
          help={t("playbooks.page.editor.help.runtime")}
        >
          {/* Runtime is pinned to Ansible: read-only, no picker. */}
          <Input
            className={fieldCls}
            value={t("playbooks.page.runtime.ansible")}
            disabled
            readOnly
          />
        </FormField>
        <FormField
          label={t("playbooks.page.fields.connectionDefault")}
          help={t("playbooks.page.editor.help.connectionDefault")}
        >
          <FormSelect
            value={form.connectionDefault}
            disabled={readOnly}
            onValueChange={(value) => update("connectionDefault", value)}
            triggerClassName={fieldCls}
            options={CONNECTIONS.map((connection) => ({
              value: connection,
              label: t(`playbooks.page.connection.${connection}`),
            }))}
          />
        </FormField>
        {isEdit ? (
          <FormField
            label={t("playbooks.page.fields.defaultTargetSet")}
            help={t("playbooks.page.editor.help.defaultTargetSet")}
          >
            <SearchableSelect
              value={form.defaultTargetSetId}
              disabled={readOnly}
              onValueChange={(value) => update("defaultTargetSetId", value)}
              placeholder={t("playbooks.page.placeholders.targetSet")}
              searchPlaceholder={t("search")}
              triggerClassName={fieldCls}
              items={[
                {
                  id: NO_TARGET_SET,
                  label: t("playbooks.page.editor.noDefaultTargetSet"),
                },
                ...targetSets.map((targetSet) => ({
                  id: targetSet.id,
                  label: `${targetSet.name} · ${targetSet.mode}`,
                  searchText: [targetSet.name, targetSet.mode].filter(Boolean).join(" "),
                })),
              ]}
            />
          </FormField>
        ) : null}
      </div>

      <FormField
        label={t("playbooks.page.fields.content")}
        required
        help={t("playbooks.page.editor.help.content")}
      >
        <CodeEditor
          value={form.content}
          readOnly={readOnly}
          onChange={(value) => update("content", value)}
          minHeight="16rem"
          aria-label={t("playbooks.page.fields.content")}
        />
      </FormField>

      <FormField
        label={t("playbooks.page.fields.varsSchema")}
        help={t("playbooks.page.editor.help.varsSchema")}
      >
        <VarsSchemaEditor
          rows={form.varsSchema}
          readOnly={readOnly}
          onChange={(rows) => update("varsSchema", rows)}
        />
      </FormField>

      <FormField
        label={t("playbooks.page.fields.secrets")}
        help={t("playbooks.page.editor.help.secrets")}
      >
        <PlaybookSecretsEditor
          rows={form.secretsEnv}
          readOnly={readOnly}
          secrets={secrets}
          secretsLoading={secretsLoading}
          onChange={(rows) => update("secretsEnv", rows)}
        />
      </FormField>

      {isEdit && !readOnly && (
        <FormField
          label={t("playbooks.page.editor.changelog")}
          help={t("playbooks.page.editor.help.changelog")}
        >
          <Textarea
            className="min-h-16 text-sm"
            value={form.changelog}
            placeholder={t("playbooks.page.editor.changelogPlaceholder")}
            onChange={(e) => update("changelog", e.target.value)}
          />
        </FormField>
      )}
    </div>
  );
}

/**
 * Routed playbook editor (S3-e). Reads the mode from the URL and fetches the
 * playbook by id, so back/reload restores the editor and it is deep-linkable:
 *
 *   /playbooks/new              create
 *   /playbooks/new?source=<id>  copy draft ("use as template")
 *   /playbooks/:id/edit         edit (read-only for non-`in_app` sources)
 *   /playbooks/:id/edit?tab=history
 */
export default function PlaybookEditorPage() {
  const { id } = useParams();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { t } = useTranslation();
  const { can } = usePermissions();

  const isNew = id === undefined || id === "";
  const sourceId = searchParams.get(PLAYBOOK_EDITOR_SOURCE_PARAM);
  const isCopy = isNew && sourceId !== null && sourceId !== "";
  const historyTab = searchParams.get(PLAYBOOK_EDITOR_TAB_PARAM) === "history";

  const fetchId = isCopy ? sourceId ?? "" : isNew ? "" : (id ?? "");
  const itemQuery = useQuery({
    queryKey: ["playbooks", "catalog", fetchId],
    queryFn: () => getPlaybook(fetchId),
    enabled: fetchId !== "",
  });

  const targetSetsQuery = useQuery({
    queryKey: ["playbooks", "target-sets", "all"],
    queryFn: () => listPlaybookTargetSets({ limit: 100, offset: 0 }),
  });
  const targetSets = useMemo(
    () => targetSetsQuery.data?.items ?? [],
    [targetSetsQuery.data],
  );

  const target: PlaybookEditorTarget | null = useMemo(() => {
    if (isCopy) {
      if (!itemQuery.data) return null;
      return {
        mode: "copyDraft",
        source: itemQuery.data,
        draft: createPlaybookCopyDraft(
          itemQuery.data,
          t("playbooks.page.duplicate.defaultName", {
            name: itemQuery.data.playbook.name,
          }),
        ),
      };
    }
    if (isNew) return { mode: "create" };
    if (!itemQuery.data) return null;
    return {
      mode: itemQuery.data.playbook.source_kind === "in_app" ? "edit" : "view",
      item: itemQuery.data,
      initialTab: historyTab ? "history" : undefined,
    };
  }, [isNew, isCopy, itemQuery.data, historyTab, t]);

  if (target === null) {
    return (
      <EntityEditorPage scope="routed" width="standard"
        title={t(isCopy ? "playbooks.page.editor.copyDraftTitle" : "playbooks.page.editor.editTitle", { name: fetchId })}
        description={t("playbooks.page.editor.description")}
        onBack={() => void navigate(playbookReturnHref(searchParams.get("returnTo")))}
        backLabel={t("back")} cancelLabel={t("cancel")}
      >
        {itemQuery.isPending ? <output className="flex justify-center p-8">
          <Loader2 className="size-6 animate-spin" aria-label={t("loading")} />
        </output> : <div role="alert" className="flex flex-col items-start gap-4">
          <p>{t(itemQuery.isError && !(itemQuery.error instanceof ApiError && itemQuery.error.status === 404) ? "unknownError" : "notFound.title")}</p>
          {itemQuery.isError && <Button variant="outline" onClick={() => void itemQuery.refetch()}>{t("retry")}</Button>}
        </div>}
      </EntityEditorPage>
    );
  }

  const canCreatePlaybook = can("CREATE", "PLAYBOOK");

  return (
    <PlaybookEditorShell
      target={target}
      targetSets={targetSets}
      onSaved={() => {
        void queryClient.invalidateQueries({ queryKey: ["playbooks"] });
      }}
      onBack={() => void navigate(playbookReturnHref(searchParams.get("returnTo")))}
      onUseAsTemplate={
        canCreatePlaybook
          ? (item) => void navigate(playbookEditorCopyHref(item.playbook.id))
          : undefined
      }
    />
  );
}
