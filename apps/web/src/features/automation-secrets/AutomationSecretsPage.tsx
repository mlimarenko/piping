import { useId, useMemo, useRef, useState } from "react";
import { useTranslation, Trans } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Loader2, Pencil, Plus, Trash2 } from "lucide-react";

import PageChrome from "@/shared/ui-shared/PageChrome";
import PageState from "@/shared/ui-shared/PageState";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import DataTable, { type DataTableSortState } from "@/shared/ui-shared/DataTable";
import { applyLocalSort } from "@/shared/ui-shared/dataTableLocal";
import { sanitizeDataTableSortState } from "@/shared/ui-shared/tableState";
import EmptyState from "@/shared/ui-shared/EmptyState";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { Button } from "@/shared/ui-primitives/button";
import { Badge } from "@/shared/ui-primitives/badge";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { notify } from "@/shared/lib/notifications";
import { formatRelativeTime } from "@/shared/lib/format";
import type { ApiError } from "@/shared/api/client";
import AutomationSecretValueDialog from "@/features/automation-secrets/AutomationSecretValueDialog";
import {
  AUTOMATION_SECRETS_QUERY_KEY,
  clearAutomationSecretValue,
  createAutomationSecret,
  deleteAutomationSecret,
  listAutomationSecrets,
  setAutomationSecretValue,
  updateAutomationSecret,
  type AutomationSecret,
} from "@/features/automation-secrets/api";
import { AUTOMATION_SECRETS_TABLE_CONFIG } from "@/features/automation-secrets/tableConfig";

/** Mirrors the backend key contract for an advisory client-side check. */
const KEY_PATTERN = /^[a-z0-9]([a-z0-9._-]{0,198}[a-z0-9])?$/;
const RESERVED_KEY_PREFIX = "secret-field/";

function validateKey(key: string): "empty" | "pattern" | "reserved" | null {
  const trimmed = key.trim();
  if (!trimmed) return "empty";
  if (trimmed.startsWith(RESERVED_KEY_PREFIX)) return "reserved";
  if (!KEY_PATTERN.test(trimmed)) return "pattern";
  return null;
}

const AutomationSecretsPage = () => {
  const { t } = useTranslation();
  const keyId = useId();
  const descriptionId = useId();
  const queryClient = useQueryClient();
  const { can } = usePermissions();
  const canCreate = can("CREATE", "SECRET_VARIABLE");
  const canUpdate = can("UPDATE", "SECRET_VARIABLE");
  const canDelete = can("DELETE", "SECRET_VARIABLE");
  // Setting/clearing a value is the SECRET action (distinct from UPDATE, which
  // only edits the description).
  const canSetValue = can("SECRET", "SECRET_VARIABLE");

  const [search, setSearch] = useState("");
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [editing, setEditing] = useState<AutomationSecret | null>(null);
  const [valueTarget, setValueTarget] = useState<AutomationSecret | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<AutomationSecret | null>(null);
  const rowTriggers = useRef(new Map<string, HTMLButtonElement>());
  const returnFocus = useRef<HTMLButtonElement | null>(null);
  const focusRow = (id: string) => {
    returnFocus.current = rowTriggers.current.get(id) ?? null;
    returnFocus.current?.focus();
  };

  const [keyInput, setKeyInput] = useState("");
  const [descInput, setDescInput] = useState("");
  const [keyError, setKeyError] = useState<string>("");

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: AUTOMATION_SECRETS_QUERY_KEY,
    queryFn: listAutomationSecrets,
  });
  const secrets = useMemo(() => data ?? [], [data]);

  const filtered = useMemo(() => {
    const query = search.trim().toLowerCase();
    if (!query) return secrets;
    return secrets.filter(
      (s) =>
        s.key.toLowerCase().includes(query) ||
        (s.description ?? "").toLowerCase().includes(query),
    );
  }, [secrets, search]);

  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, AUTOMATION_SECRETS_TABLE_CONFIG.sortKeys),
    [sortState],
  );
  // All sortable keys are plain row fields, so no per-column getSortValue.
  const rows = useMemo(
    () => applyLocalSort(filtered, [], sanitizedSortState),
    [filtered, sanitizedSortState],
  );

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: AUTOMATION_SECRETS_QUERY_KEY });

  const createMutation = useMutation({
    mutationFn: () =>
      createAutomationSecret({
        key: keyInput.trim(),
        description: descInput.trim() || null,
      }),
    onSuccess: async () => {
      await invalidate();
      notify("success", t("automationSecrets.toasts.created"));
      setCreateOpen(false);
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const updateMutation = useMutation({
    mutationFn: (vars: { id: string; description: string | null }) =>
      updateAutomationSecret(vars.id, { description: vars.description }),
    onSuccess: async () => {
      await invalidate();
      notify("success", t("automationSecrets.toasts.updated"));
      setEditing(null);
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteMutation = useMutation({
    mutationFn: (id: string) => deleteAutomationSecret(id),
    onSuccess: async () => {
      await invalidate();
      notify("success", t("automationSecrets.toasts.deleted"));
      setDeleteTarget(null);
    },
    onError: (err: ApiError) => {
      // A 409 means the secret is still referenced by a playbook version; keep
      // the confirm open and explain, rather than a generic failure toast.
      const message =
        err.status === 409
          ? t("automationSecrets.errors.inUse")
          : err.message || t("unknownError");
      notify("error", message);
      setDeleteTarget(null);
    },
  });

  const openCreate = () => {
    setKeyInput("");
    setDescInput("");
    setKeyError("");
    setCreateOpen(true);
  };
  const openEdit = (secret: AutomationSecret) => {
    setDescInput(secret.description ?? "");
    setEditing(secret);
  };

  const submitCreate = () => {
    if (createMutation.isPending) return;
    const error = validateKey(keyInput);
    if (error) {
      setKeyError(t(`automationSecrets.keyErrors.${error}`));
      return;
    }
    setKeyError("");
    createMutation.mutate();
  };

  const columns = [
    {
      key: "key",
      label: t("automationSecrets.fields.key"),
      render: (s: AutomationSecret) => (
        <span className="font-mono text-sm font-medium text-foreground">{s.key}</span>
      ),
    },
    {
      key: "description",
      label: t("automationSecrets.fields.description"),
      render: (s: AutomationSecret) =>
        s.description ? (
          <span className="block max-w-[36ch] truncate text-muted-foreground" title={s.description}>
            {s.description}
          </span>
        ) : (
          <span className="text-muted-foreground/60">—</span>
        ),
    },
    {
      key: "has_value",
      label: t("automationSecrets.fields.value"),
      render: (s: AutomationSecret) =>
        s.has_value ? (
          <Badge variant="status-active" className="rounded text-xs">
            {t("automationSecrets.value.set")}
          </Badge>
        ) : (
          <Badge variant="status-staging" className="rounded text-xs">
            {t("automationSecrets.value.unset")}
          </Badge>
        ),
    },
    {
      key: "usage_count",
      label: t("automationSecrets.fields.usage"),
      render: (s: AutomationSecret) => (
        <span className="text-sm text-muted-foreground">{s.usage_count}</span>
      ),
    },
    {
      key: "created_at",
      label: t("automationSecrets.fields.created"),
      render: (s: AutomationSecret) => (
        <span className="text-sm text-muted-foreground" title={s.created_at}>
          {formatRelativeTime(s.created_at)}
        </span>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      render: (s: AutomationSecret) => (canSetValue || canUpdate || canDelete) ? (
        <RowActions label={`${t("actions")}: ${s.key}`} ref={(node) => { if (node) rowTriggers.current.set(s.id, node); else rowTriggers.current.delete(s.id); }}>
          {canSetValue && (
            <DropdownMenuItem
              onSelect={() => { focusRow(s.id); setValueTarget(s); }}
            >
              <KeyRound className="h-3.5 w-3.5" />
              {t("automationSecrets.actions.setValue")}
            </DropdownMenuItem>
          )}
          {canUpdate && (
            <DropdownMenuItem
              onSelect={() => { focusRow(s.id); openEdit(s); }}
            >
              <Pencil className="h-3.5 w-3.5" />
              {t("edit")}
            </DropdownMenuItem>
          )}
          {canDelete && (
            <DropdownMenuItem variant="destructive"
              onSelect={() => { focusRow(s.id); setDeleteTarget(s); }}
            >
              <Trash2 className="h-3.5 w-3.5" />
              {t("delete")}
            </DropdownMenuItem>
          )}
        </RowActions>
      ) : null,
    },
  ];

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <PageChrome
        title={t("automationSecrets.title")}
        description={t("automationSecrets.description")}
        icon={KeyRound}
        badge={secrets.length > 0 ? String(secrets.length) : undefined}
        actions={
          canCreate ? (
            <Button onClick={openCreate}>
              <Plus className="mr-1.5 h-4 w-4" />
              {t("automationSecrets.add")}
            </Button>
          ) : undefined
        }
      />

      <DataTable
        tableId="automation-secrets.list"
        columns={columns}
        data={rows}
        sortState={sanitizedSortState}
        onSortChange={setSortState}
        getRowKey={(s) => s.id}
        loading={isPending}
        error={isError ? error : undefined}
        onRetry={() => refetch()}
        emptyState={
          isPending ? (
            <PageState kind="loading" title={t("loading")} />
          ) : (
            <EmptyState
              icon={KeyRound}
              title={t("automationSecrets.empty.title")}
              description={t("automationSecrets.empty.description")}
              actionLabel={canCreate ? t("automationSecrets.add") : undefined}
              onAction={canCreate ? openCreate : undefined}
            />
          )
        }
        toolbar={
          <SearchInput
            value={search}
            onValueChange={setSearch}
            placeholder={t("automationSecrets.searchPlaceholder")}
          />
        }
      />

      <EntityEditorHost open={createOpen} onOpenChange={(open) => { if (!createMutation.isPending) setCreateOpen(open); }}
        title={t("automationSecrets.create.title")} description={t("automationSecrets.create.description")}
        dirty={keyInput !== "" || descInput !== ""}
        footer={<>
          <Button data-editor-cancel variant="outline" disabled={createMutation.isPending} onClick={() => setCreateOpen(false)}>{t("cancel")}</Button>
          <Button disabled={createMutation.isPending} onClick={submitCreate}>{createMutation.isPending && <Loader2 className="size-4 animate-spin" />}{t("create")}</Button>
        </>}>
          <fieldset disabled={createMutation.isPending} inert={createMutation.isPending} className="flex min-w-0 flex-col gap-4">
            <div className="flex flex-col gap-2">
              <FieldLabel htmlFor={keyId} required className="text-sm">
                {t("automationSecrets.fields.key")}
              </FieldLabel>
              <Input
                id={keyId}
                className="h-9 font-mono text-sm"
                value={keyInput}
                autoFocus
                placeholder="grad-api-token"
                aria-invalid={keyError ? true : undefined}
                onChange={(e) => {
                  if (createMutation.isPending) return;
                  setKeyInput(e.target.value);
                  setKeyError("");
                }}
              />
              {keyError ? (
                <p className="text-xs text-destructive">{keyError}</p>
              ) : (
                <p className="text-xs text-muted-foreground">
                  {t("automationSecrets.create.keyHint")}
                </p>
              )}
            </div>
            <div className="flex flex-col gap-2">
              <FieldLabel htmlFor={descriptionId} className="text-sm">{t("automationSecrets.fields.description")}</FieldLabel>
              <Textarea
                id={descriptionId}
                className="min-h-16 text-sm"
                value={descInput}
                placeholder={t("automationSecrets.create.descPlaceholder")}
                onChange={(e) => { if (!createMutation.isPending) setDescInput(e.target.value); }}
              />
            </div>
          </fieldset>
      </EntityEditorHost>

      <EntityEditorHost open={editing !== null} onOpenChange={(open) => { if (!open && !updateMutation.isPending) setEditing(null); }}
        title={t("automationSecrets.edit.title", { key: editing?.key ?? "" })} description={t("automationSecrets.edit.description")}
        dirty={descInput !== (editing?.description ?? "")}
        footer={<>
          <Button data-editor-cancel variant="outline" disabled={updateMutation.isPending} onClick={() => setEditing(null)}>{t("cancel")}</Button>
          <Button disabled={updateMutation.isPending} onClick={() => editing && updateMutation.mutate({ id: editing.id, description: descInput.trim() || null })}>
            {updateMutation.isPending && <Loader2 className="size-4 animate-spin" />}{t("saveChanges")}
          </Button>
        </>}>
          <fieldset disabled={updateMutation.isPending} inert={updateMutation.isPending} className="flex min-w-0 flex-col gap-2">
            <FieldLabel htmlFor={descriptionId} className="text-sm">{t("automationSecrets.fields.description")}</FieldLabel>
            <Textarea
              id={descriptionId}
              className="min-h-16 text-sm"
              value={descInput}
              onChange={(e) => { if (!updateMutation.isPending) setDescInput(e.target.value); }}
            />
          </fieldset>
      </EntityEditorHost>

      <AutomationSecretValueDialog
        secret={valueTarget}
        open={valueTarget !== null}
        onOpenChange={(open) => !open && setValueTarget(null)}
        onSetValue={async (value) => {
          if (!valueTarget) return;
          await setAutomationSecretValue(valueTarget.id, value);
          await invalidate();
          notify("success", t("automationSecrets.toasts.valueSet"));
        }}
        onClearValue={async () => {
          if (!valueTarget) return;
          await clearAutomationSecretValue(valueTarget.id);
          await invalidate();
          notify("success", t("automationSecrets.toasts.valueCleared"));
        }}
      />

      <AlertDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      >
        <AlertDialogContent onCloseAutoFocus={(event) => { event.preventDefault(); returnFocus.current?.focus(); }} className="max-h-[90dvh] overflow-y-auto bg-card">
          <AlertDialogHeader>
            <AlertDialogTitle>{t("automationSecrets.delete.title")}</AlertDialogTitle>
            <AlertDialogDescription>
              <Trans
                i18nKey="automationSecrets.delete.description"
                values={{ key: deleteTarget?.key ?? "" }}
                components={{ 1: <strong /> }}
              />
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel className="h-9">{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className="h-9 bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={(e) => {
                e.preventDefault();
                if (deleteTarget) deleteMutation.mutate(deleteTarget.id);
              }}
            >
              {t("delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
};

export default AutomationSecretsPage;
