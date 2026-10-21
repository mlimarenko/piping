import { useMemo, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Check, ChevronRight, Copy } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import FormSection from "@/shared/ui-shared/FormSection";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import PermissionPicker from "@/shared/ui-shared/PermissionPicker";
import { TogglePillGroup } from "@/shared/ui-shared/TogglePill";
import EntityTargetPicker from "@/features/user-settings/EntityTargetPicker";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "@/shared/ui-primitives/dialog";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/shared/ui-primitives/collapsible";
import { writeBrowserClipboardText } from "@/shared/lib/clipboard";
import { notify } from "@/shared/lib/notifications";
import { me } from "@/features/auth/api";
import { createMyToken, listMyTokens, listTokenScopes } from "@/features/user-settings/api";
import {
  computeScopePacks,
  distinctScopeValues,
  groupScopesByResource,
  isSubsetSelected,
  manageScopeValues,
  mergeTokenScopeOptions,
  patternsForScope,
  readScopeValues,
  reconcileSelectedScopes,
  resourceDomain,
  setScopeTargetRows,
  tokenScopeCatalogEntry,
  toggleBundle,
} from "@/features/user-settings/tokenScopes";
import type { ApiToken } from "@/shared/api/types";
import { userSettingsSectionHref } from "./userSettingsRoutes";

interface TokenScopeRow {
  id: string;
  scope: string;
  resourcePattern: string;
}

interface TokenForm {
  name: string;
  scopes: TokenScopeRow[];
  expiresInSeconds: string;
}

let tokenScopeRowId = 0;

function createTokenScopeRow(scope = "read:inventory", resourcePattern = "*"): TokenScopeRow {
  tokenScopeRowId += 1;
  return {
    id: `token-scope-${tokenScopeRowId}`,
    scope,
    resourcePattern,
  };
}

function createEmptyTokenForm(): TokenForm {
  return {
    name: "",
    scopes: [],
    expiresInSeconds: "",
  };
}

function formatTokenScopeRow(row: TokenScopeRow): string {
  const scope = row.scope.trim();
  const pattern = row.resourcePattern.trim();
  return pattern ? `${scope}@${pattern}` : scope;
}

export default function UserTokenEditor() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const nameInput = useRef<HTMLInputElement>(null);
  const expiryInput = useRef<HTMLInputElement>(null);
  const permissions = useRef<HTMLDivElement>(null);
  const qc = useQueryClient();
  const [baseline] = useState<TokenForm>(() => createEmptyTokenForm());
  const [form, setForm] = useState<TokenForm>(baseline);
  const [submitAttempted, setSubmitAttempted] = useState(false);
  const [advancedTargetingOpen, setAdvancedTargetingOpen] = useState(false);
  const [revealedToken, setRevealedToken] = useState<ApiToken | null>(null);
  const [tokenCopied, setTokenCopied] = useState(false);

  const back = () => { void navigate(userSettingsSectionHref("tokens", location)); };

  const meQuery = useQuery({ queryKey: ["auth", "me"], queryFn: me });
  const tokenScopesQuery = useQuery({
    queryKey: ["user", "token-scope-catalog"],
    queryFn: listTokenScopes,
  });
  const tokensQuery = useQuery({
    queryKey: ["user", "tokens", "options"],
    queryFn: () => listMyTokens({ limit: 1000, offset: 0 }),
  });
  const tokens = useMemo(() => tokensQuery.data?.items ?? [], [tokensQuery.data]);

  const observedTokenScopeOptions = useMemo(() => {
    const values = new Set<string>();
    for (const token of tokens) {
      for (const scope of token.scopes.split(/[,\s]+/).filter(Boolean)) {
        const entry = tokenScopeCatalogEntry(scope);
        if (entry) values.add(entry.value);
      }
    }
    for (const row of form.scopes) {
      const entry = tokenScopeCatalogEntry(row.scope);
      if (entry) values.add(entry.value);
    }
    return Array.from(values)
      .map((value) => tokenScopeCatalogEntry(value))
      .filter((entry): entry is NonNullable<typeof entry> => entry !== null);
  }, [form.scopes, tokens]);
  const tokenScopeOptions = useMemo(
    () => mergeTokenScopeOptions(tokenScopesQuery.data ?? [], observedTokenScopeOptions),
    [observedTokenScopeOptions, tokenScopesQuery.data],
  );

  const currentUser = meQuery.data;
  const scopeResourceType = (scopeValue: string): string => {
    const match = tokenScopeOptions.find((opt) => opt.value === scopeValue);
    if (match) return match.resourceType;
    const sep = scopeValue.indexOf(":");
    return sep > 0 ? scopeValue.slice(sep + 1) : "";
  };

  const scopeGroups = groupScopesByResource(tokenScopeOptions);
  const scopePacks = computeScopePacks(tokenScopeOptions);
  const grantableScopeValues = useMemo(
    () =>
      new Set(
        (currentUser?.capabilities ?? []).map(
          (capability) => `${capability.action.toLowerCase()}:${capability.resourceType.toLowerCase()}`,
        ),
      ),
    [currentUser?.capabilities],
  );
  const isScopeGrantable = (scopeValue: string): boolean => grantableScopeValues.has(scopeValue);
  const grantableOnly = (values: string[]): string[] => values.filter(isScopeGrantable);
  const grantableScopePacks = scopePacks
    .map((pack) => ({ ...pack, scopes: grantableOnly(pack.scopes) }))
    .filter((pack) => pack.scopes.length > 0);
  const packLabels: Record<string, string> = {
    readOnly: t("userSettings.packReadOnly"),
    connectionsOperator: t("userSettings.packConnectionsOperator"),
    inventoryManager: t("userSettings.packInventoryManager"),
    playbookOperator: t("userSettings.packPlaybookOperator"),
    playbookManager: t("userSettings.packPlaybookManager"),
    accessAdmin: t("userSettings.packAccessAdmin"),
    fullAccess: t("userSettings.packFullAccess"),
  };

  const selectedScopeValues = new Set(form.scopes.map((row) => row.scope));

  const applySelectedScopes = (nextValues: string[]) => {
    setForm((current) => ({
      ...current,
      scopes: reconcileSelectedScopes(current.scopes, nextValues, (scope, pattern) =>
        createTokenScopeRow(scope, pattern),
      ),
    }));
  };

  const setScopeTargets = (scopeValue: string, patterns: string[]) => {
    setForm((current) => ({
      ...current,
      scopes: setScopeTargetRows(current.scopes, scopeValue, patterns, (scope, pattern) =>
        createTokenScopeRow(scope, pattern),
      ),
    }));
  };

  const toggleQuickScope = (scopeValue: string) => {
    if (!isScopeGrantable(scopeValue)) return;
    const current = form.scopes.map((row) => row.scope);
    applySelectedScopes(toggleBundle([scopeValue], current));
  };

  const toggleScopeBundle = (values: string[]) => {
    const grantableValues = grantableOnly(values);
    if (grantableValues.length === 0) return;
    const current = form.scopes.map((row) => row.scope);
    applySelectedScopes(toggleBundle(grantableValues, current));
  };

  const setScopesSelected = (values: string[], selected: boolean) => {
    const grantableValues = grantableOnly(values);
    if (grantableValues.length === 0) return;
    const next = new Set(form.scopes.map((row) => row.scope));
    for (const value of grantableValues) {
      if (selected) next.add(value);
      else next.delete(value);
    }
    applySelectedScopes([...next]);
  };

  const clearAllScopes = () =>
    setForm((current) => ({ ...current, scopes: [] }));

  const tokenCreate = useApiMutation({
    mutationFn: async (body: TokenForm) => {
      const token = await createMyToken({
        name: body.name.trim(),
        scopes: body.scopes.map(formatTokenScopeRow).join(" "),
        expiresInSeconds: body.expiresInSeconds.trim()
          ? Number(body.expiresInSeconds.trim())
          : undefined,
      });
      // Plaintext belongs only to this mounted reveal, never the mutation cache.
      setTokenCopied(false);
      setRevealedToken(token);
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["user", "tokens"] });
    },
  });

  const handleTokenCreate = () => {
    if (tokenCreate.isPending || revealedToken) return;
    setSubmitAttempted(true);
    if (!form.name.trim()) {
      nameInput.current?.focus();
      return;
    }
    if (!form.scopes.length || form.scopes.some((row) => !isScopeGrantable(row.scope))) {
      permissions.current?.focus();
      return;
    }
    if (form.scopes.some((row) => /\s/.test(row.resourcePattern.trim()))) {
      setAdvancedTargetingOpen(true);
      permissions.current?.focus();
      return;
    }
    if (
      form.expiresInSeconds.trim() &&
      (!Number.isInteger(Number(form.expiresInSeconds.trim())) ||
        Number(form.expiresInSeconds.trim()) <= 0)
    ) {
      expiryInput.current?.focus();
      return;
    }
    tokenCreate.mutate(form);
  };

  const handleCopyRevealedToken = async () => {
    const plaintext = revealedToken?.plaintext;
    if (!plaintext) return;

    try {
      await writeBrowserClipboardText(plaintext);
      notify("success", t("copied"));
      setTokenCopied(true);
    } catch {
      notify("error", t("userSettings.tokenRevealCopyFailed"));
    }
  };

  const expiryPresets = [
    { key: "none", seconds: "", label: t("userSettings.expiryPresetNone") },
    { key: "7d", seconds: "604800", label: t("userSettings.expiryPreset7d") },
    { key: "30d", seconds: "2592000", label: t("userSettings.expiryPreset30d") },
    { key: "90d", seconds: "7776000", label: t("userSettings.expiryPreset90d") },
    { key: "1y", seconds: "31536000", label: t("userSettings.expiryPreset1y") },
    { key: "custom", seconds: null, label: t("userSettings.expiryPresetCustom") },
  ] as const;
  const matchedPreset = expiryPresets.find(
    (p) => p.seconds !== null && p.seconds === form.expiresInSeconds.trim(),
  );
  const activePresetKey = matchedPreset?.key ?? (form.expiresInSeconds.trim() ? "custom" : "none");
  const nameError = submitAttempted && !form.name.trim() ? t("userSettings.nameRequired") : null;
  const scopeError = !submitAttempted ? null
    : !form.scopes.length ? t("userSettings.scopeRequired")
    : form.scopes.some((row) => !isScopeGrantable(row.scope)) ? t("userSettings.scopeUnavailable")
    : form.scopes.some((row) => /\s/.test(row.resourcePattern.trim())) ? t("userSettings.scopePatternInvalid") : null;
  const expiryError = submitAttempted && form.expiresInSeconds.trim() &&
    (!Number.isInteger(Number(form.expiresInSeconds)) || Number(form.expiresInSeconds) <= 0)
    ? t("userSettings.expiresInSecondsInvalid") : null;

  return (
    <>
      <EntityEditorPage scope="routed"
        title={t("userSettings.createApiToken")}
        onBack={back}
        backLabel={t("back")}
        onCancel={back}
        cancelLabel={t("cancel")}
        saving={tokenCreate.isPending}
        dirty={!tokenCreate.isSuccess && JSON.stringify(form) !== JSON.stringify(baseline)}
        onSave={handleTokenCreate}
        saveLabel={t("create")}
        saveDisabled={!!revealedToken || tokenCreate.isSuccess}
        error={tokenCreate.isError ? <p role="alert">{t("userSettings.tokenCreationFailed")}</p> : undefined}
      >
        <div className="flex flex-col gap-6">
          {/* Section 1: Name */}
          <FormSection
            title={t("userSettings.nameSectionTitle")}
          >
            <FormField htmlFor="api-token-name" label={t("userSettings.nameSectionTitle")} labelClassName="sr-only" required
              help={t("userSettings.nameSectionHelper")} error={nameError}>
              <FormFieldControl>
              <Input
                id="api-token-name"
                ref={nameInput}
                aria-required="true"
                placeholder={t("userSettings.namePlaceholder")}
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
              />
              </FormFieldControl>
            </FormField>
          </FormSection>

          {/* Section 2: Permissions */}
          <FormSection
            divider
            title={t("userSettings.permissionsSectionTitle")}
            description={t("userSettings.permissionsSectionHelper")}
            contentClassName="space-y-4"
          >
            <FormField error={scopeError}>
            <FormFieldControl>
            <div ref={permissions} tabIndex={-1} role="group" aria-label={t("userSettings.permissionsSectionTitle")}>
            <PermissionPicker
              groups={scopeGroups.map((group) => ({
                key: group.resourceType,
                title: String(
                  t(`accessControl.policies.resourceTypes.${group.resourceType}`, {
                    defaultValue: group.resourceType,
                  }),
                ),
                section: String(
                  t(`accessControl.policies.actionGroups.${resourceDomain(group.resourceType)}`, {
                    defaultValue: resourceDomain(group.resourceType),
                  }),
                ),
                items: group.scopes.map((scope) => {
                  const friendly = String(
                    t(`accessControl.policies.actionLabels.${scope.action}`, {
                      defaultValue: scope.action,
                    }),
                  );
                  const desc = String(
                    t(`accessControl.policies.actionDescriptions.${scope.action}`, {
                      defaultValue: "",
                    }),
                  );
                  return {
                    value: scope.value,
                    label: friendly,
                    hint: scope.action,
                    description: desc ? `${friendly} — ${desc}` : friendly,
                    disabled: !isScopeGrantable(scope.value),
                    disabledReason: t("userSettings.scopeUnavailable"),
                  };
                }),
              }))}
              selectedValues={selectedScopeValues}
              onToggleValue={toggleQuickScope}
              onToggleValues={setScopesSelected}
              countLabel={t("userSettings.scopeSelectedSummary", { count: selectedScopeValues.size })}
              hasSelection={selectedScopeValues.size > 0}
              onClearAll={clearAllScopes}
              clearAllLabel={t("userSettings.permissionsClearAll")}
              bundles={grantableScopePacks.map((pack) => ({
                id: pack.id,
                label: packLabels[pack.id] ?? pack.id,
                values: pack.scopes,
              }))}
              bundlesLabel={t("userSettings.packsSectionTitle")}
              isBundleActive={(bundle) => isSubsetSelected(bundle.values, selectedScopeValues)}
              onToggleBundle={(bundle) => toggleScopeBundle(bundle.values)}
              emptyLabel={t("noResults")}
              groupClassName="p-3"
              renderGroupActions={(group) => {
                const sourceGroup = scopeGroups.find(scopeGroup => scopeGroup.resourceType === group.key);
                if (!sourceGroup) return null;
                const readValues = grantableOnly(readScopeValues(sourceGroup.scopes));
                const manageValues = grantableOnly(manageScopeValues(sourceGroup.scopes));
                const readActive = isSubsetSelected(readValues, selectedScopeValues);
                const manageActive = isSubsetSelected(manageValues, selectedScopeValues);

                return (
                  <>
                    {readValues.length > 0 && (
                      <Button variant="ghost" size="sm"
                        type="button"
                        aria-pressed={readActive}
                        onClick={() => toggleScopeBundle(readValues)}
                        className={`h-9 rounded-md border px-3 text-sm font-medium transition-colors ${
                          readActive
                            ? "border-border bg-accent text-primary"
                            : "border-input/30 text-muted-foreground hover:border-input/60 hover:text-foreground"
                        }`}
                      >
                        {t("userSettings.resourceReadToggle")}
                      </Button>
                    )}
                    {manageValues.length > 0 && (
                      <Button variant="ghost" size="sm"
                        type="button"
                        aria-pressed={manageActive}
                        onClick={() => toggleScopeBundle(manageValues)}
                        className={`h-9 rounded-md border px-3 text-sm font-medium transition-colors ${
                          manageActive
                            ? "border-border bg-accent text-primary"
                            : "border-input/30 text-muted-foreground hover:border-input/60 hover:text-foreground"
                        }`}
                      >
                        {t("userSettings.resourceManageToggle")}
                      </Button>
                    )}
                  </>
                );
              }}
            />

            </div>
            </FormFieldControl>
            </FormField>

            {/* Advanced: per-scope resource targeting */}
            <Collapsible open={advancedTargetingOpen} onOpenChange={setAdvancedTargetingOpen}>
              <CollapsibleTrigger asChild>
                <Button variant="ghost" size="sm"
                  type="button"
                  className="h-9 flex w-full justify-start items-center gap-2 text-sm text-muted-foreground/80 hover:text-foreground select-none"
                >
                  <ChevronRight
                    className={`h-3.5 w-3.5 transition-transform ${
                      advancedTargetingOpen ? "rotate-90" : ""
                    }`}
                  />
                  {t("userSettings.advancedTargetingTitle")}
                </Button>
              </CollapsibleTrigger>
              <CollapsibleContent className="pt-2.5">
                <p className="mb-2 text-xs text-muted-foreground">
                  {t("userSettings.advancedTargetingHelper")}
                </p>
                {form.scopes.length === 0 ? (
                  <p className="rounded-md border border-border/30 bg-card p-2.5 text-xs text-muted-foreground">
                    {t("userSettings.advancedTargetingEmpty")}
                  </p>
                ) : (
                  <div className="space-y-1.5">
                    {distinctScopeValues(form.scopes).map((scopeValue) => (
                      <div
                        key={scopeValue}
                        className="grid grid-cols-1 items-start gap-2 rounded-md border border-border/30 bg-card p-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)]"
                      >
                        <code className="min-w-0 truncate pt-1.5 font-mono text-xs text-foreground">
                          {scopeValue}
                        </code>
                        <EntityTargetPicker
                          resourceType={scopeResourceType(scopeValue)}
                          value={patternsForScope(form.scopes, scopeValue)}
                          onChange={(patterns) => setScopeTargets(scopeValue, patterns)}
                        />
                      </div>
                    ))}
                  </div>
                )}
              </CollapsibleContent>
            </Collapsible>
          </FormSection>

          {/* Section 3: Expiration */}
          <FormSection
            divider
            title={t("userSettings.expirationSectionTitle")}
            description={t("userSettings.expirationSectionHelper")}
            contentClassName="space-y-3"
          >
            <TogglePillGroup
              shape="md"
              value={activePresetKey}
              onValueChange={(key) => {
                const preset = expiryPresets.find((p) => p.key === key);
                if (!preset) return;
                if (preset.seconds !== null) {
                  setForm({ ...form, expiresInSeconds: preset.seconds });
                } else if (!form.expiresInSeconds.trim()) {
                  setForm({ ...form, expiresInSeconds: "3600" });
                }
              }}
              options={expiryPresets.map((preset) => ({ value: preset.key, label: preset.label }))}
            />
            {activePresetKey === "custom" && (
              <FormField htmlFor="api-token-expires-custom" label={t("userSettings.expiryCustomSecondsLabel")}
                error={expiryError}>
                <FormFieldControl>
                <Input
                  id="api-token-expires-custom"
                  ref={expiryInput}
                  className="w-40"
                  placeholder={t("userSettings.expiresPlaceholder")}
                  value={form.expiresInSeconds}
                  onChange={(e) =>
                    setForm({ ...form, expiresInSeconds: e.target.value })
                  }
                  type="number"
                  min="1"
                />
                </FormFieldControl>
              </FormField>
            )}
            <p className="text-xs text-muted-foreground/80">
              {form.expiresInSeconds.trim()
                ? t("userSettings.expiryResolvedOn", {
                    duration: (() => {
                      const s = Number(form.expiresInSeconds.trim());
                      if (!s || s <= 0) return form.expiresInSeconds;
                      if (s < 3600) return `${Math.round(s / 60)}m`;
                      if (s < 86400) return `${Math.round(s / 3600)}h`;
                      if (s < 31536000) return `${Math.round(s / 86400)}d`;
                      return `${Math.round(s / 31536000)}y`;
                    })(),
                  })
                : t("userSettings.expiryResolvedNoExpiry")}
            </p>
          </FormSection>
        </div>
      </EntityEditorPage>

      {/* ── Reveal token dialog ── */}
      <Dialog
        open={!!revealedToken}
        onOpenChange={(o) => {
          if (!o) {
            setRevealedToken(null);
            setTokenCopied(false);
          }
        }}
      >
        <DialogContent className="max-h-[90dvh] overflow-y-auto bg-card sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>{t("userSettings.tokenCreated")}</DialogTitle>
          </DialogHeader>
          <div className="space-y-4 py-1">
            <div className="flex items-start gap-2.5 rounded-lg border border-amber-500/30 bg-muted px-3.5 py-3">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-500" />
              <p className="text-xs leading-relaxed text-status-warning">
                {t("userSettings.tokenRevealWarning")}
              </p>
            </div>

            <div className="space-y-2">
              <div className="rounded-md border border-border/40 bg-card px-3 py-2.5">
                <p className="break-all font-mono text-sm text-foreground select-all leading-relaxed">
                  {revealedToken?.plaintext ?? ""}
                </p>
              </div>
              <Button
                className="w-full gap-2"
                size="sm"
                variant={tokenCopied ? "outline" : "default"}
                onClick={() => {
                  void handleCopyRevealedToken();
                }}
              >
                {tokenCopied ? (
                  <>
                    <Check className="h-3.5 w-3.5" />
                    {t("userSettings.tokenRevealCopied")}
                  </>
                ) : (
                  <>
                    <Copy className="h-3.5 w-3.5" />
                    {t("userSettings.tokenRevealCopy")}
                  </>
                )}
              </Button>
            </div>
          </div>
          <DialogFooter>
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                setRevealedToken(null);
                setTokenCopied(false);
                back();
              }}
            >
              {t("userSettings.tokenRevealDone")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
