import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router";
import { useTranslation, Trans } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTablePageSize } from "@/shared/hooks/useTablePageSize";
import { useApiMutation } from "@/shared/api/hooks";
import PageChrome, { PageActions } from "@/shared/ui-shared/PageChrome";
import SurfaceCard, { SurfaceCardHeader } from "@/shared/ui-shared/SurfaceCard";
import RowActions from "@/shared/ui-shared/RowActions";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import StatusChip from "@/shared/ui-shared/StatusChip";
import SearchInput from "@/shared/ui-shared/SearchInput";
import SettingsCatalog from "@/features/settings/SettingsCatalog";
import { CONFIG_CONTEXT_FORMAT_SETTING_KEY } from "@/shared/settings/registry";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import ConfirmDeleteDialog from "@/shared/ui-shared/ConfirmDeleteDialog";
import { mergeTokenScopeOptions, tokenScopeCatalogEntry } from "@/features/user-settings/tokenScopes";
import DataTable, { type Column, type DataTableSortState } from "@/shared/ui-shared/DataTable";
import { boolFilterOptions } from "@/shared/ui-shared/dataTableLocal";
import {
  mapDataTableSortKey,
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import {
  buildUserCredentialFilterParams,
  buildUserTokenFilterParams,
  USER_CREDENTIALS_TABLE_CONFIG,
  USER_TOKENS_TABLE_CONFIG,
} from "@/features/user-settings/tableConfig";
import EmptyState from "@/shared/ui-shared/EmptyState";
import {
  Plus,
  Pencil,
  Trash2,
  Key,
  KeyRound,
  Star,
  Shield,
  Lock,
  Palette,
  Monitor as MonitorIcon,
  Loader2,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import { me } from "@/features/auth/api";
import {
  changeMyPassword,
  deleteMyCredential,
  listMyCredentials,
  listMyTokens,
  listTokenScopes,
  revokeMyToken,
  setDefaultMyCredential,
} from "@/features/user-settings/api";
import type { ApiToken, CredentialProtocol, PersonalCredential } from "@/shared/api/types";
import {
  USER_CREDENTIAL_NEW_ROUTE,
  USER_TOKEN_NEW_ROUTE,
  userCredentialEditHref,
  userSettingsEditorHref,
} from "./userSettingsRoutes";

const PROTOCOLS: CredentialProtocol[] = ["SSH", "RDP", "VNC"];


type UserSettingsSectionId = "profile" | "interface" | "connections" | "credentials" | "tokens";

const USER_SETTINGS_SECTION_IDS: UserSettingsSectionId[] = [
  "profile",
  "interface",
  "connections",
  "credentials",
  "tokens",
];

interface UserSettingsSection {
  id: UserSettingsSectionId;
  title: string;
  description: string;
  Icon: LucideIcon;
  badge?: string;
}

interface PasswordForm {
  currentPassword: string;
  newPassword: string;
  confirmPassword: string;
}

type PasswordField = keyof PasswordForm;
const PASSWORD_FIELDS: PasswordField[] = ["currentPassword", "newPassword", "confirmPassword"];
type PasswordErrors = Partial<Record<PasswordField, string>>;

function UserSettingsSectionHeader({
  section,
}: {
  section: UserSettingsSection;
}) {
  const Icon = section.Icon;
  return (
    <SurfaceCardHeader title={<span className="flex items-center gap-2"><Icon className="size-4 text-primary" aria-hidden="true" />{section.title}</span>} description={section.description} />
  );
}

/** Format a session-expiry ISO timestamp as a short, locale-aware date+time
 *  (e.g. "14 июл 2026, 16:58") instead of dumping the raw backend ISO string. */
const formatSessionExpiry = (iso: string, locale: string): string => {
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return iso;
  return new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(parsed);
};

const UserSettings = () => {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const { t, i18n } = useTranslation();
  const rowTriggers = useRef(new Map<string, HTMLButtonElement>());
  const returnFocus = useRef<HTMLButtonElement | null>(null);
  const [searchParams] = useSearchParams();
  const location = useLocation();

  const meQuery = useQuery({ queryKey: ["auth", "me"], queryFn: me });
  const [credentialsSortState, setCredentialsSortState] = useState<DataTableSortState | null>(null);
  const [credentialsColumnFilters, setCredentialsColumnFilters] =
    useState<Record<string, string[]>>({});
  const sanitizedCredentialsSortState = useMemo(
    () =>
      sanitizeDataTableSortState(
        credentialsSortState,
        USER_CREDENTIALS_TABLE_CONFIG.sortKeys,
      ),
    [credentialsSortState],
  );
  const sanitizedCredentialsColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        credentialsColumnFilters,
        USER_CREDENTIALS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [credentialsColumnFilters],
  );
  const credentialFilterParams = useMemo(
    () => buildUserCredentialFilterParams(sanitizedCredentialsColumnFilters),
    [sanitizedCredentialsColumnFilters],
  );
  const credsQuery = useQuery({
    queryKey: [
      "user",
      "credentials",
      { sort: sanitizedCredentialsSortState, filters: credentialFilterParams },
    ],
    queryFn: () =>
      listMyCredentials({
        sort: mapDataTableSortKey(
          sanitizedCredentialsSortState,
          USER_CREDENTIALS_TABLE_CONFIG.sortKeyByColumn,
        ),
        order: sanitizedCredentialsSortState?.dir,
        protocol: credentialFilterParams.protocol,
        is_default: credentialFilterParams.is_default,
      }),
    placeholderData: (prev) => prev,
  });

  const [tokensPage, setTokensPage] = useState(0);
  const { value: tokensPageSize, setValue: setTokensPageSize } = useTablePageSize();
  const [tokensSortState, setTokensSortState] = useState<DataTableSortState | null>(null);
  const [tokensColumnFilters, setTokensColumnFilters] = useState<Record<string, string[]>>({});
  const sanitizedTokensSortState = useMemo(
    () => sanitizeDataTableSortState(tokensSortState, USER_TOKENS_TABLE_CONFIG.sortKeys),
    [tokensSortState],
  );
  const sanitizedTokensColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        tokensColumnFilters,
        USER_TOKENS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [tokensColumnFilters],
  );
  const tokenFilterParams = useMemo(
    () => buildUserTokenFilterParams(sanitizedTokensColumnFilters),
    [sanitizedTokensColumnFilters],
  );
  const [tokensSearch, setTokensSearch] = useState("");
  const [debouncedTokensSearch, setDebouncedTokensSearch] = useState("");
  const tokensSearchTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (tokensSearchTimer.current !== null) clearTimeout(tokensSearchTimer.current);
    tokensSearchTimer.current = setTimeout(() => {
      setDebouncedTokensSearch(tokensSearch.trim());
      setTokensPage(0);
    }, 250);
    return () => {
      if (tokensSearchTimer.current !== null) clearTimeout(tokensSearchTimer.current);
    };
  }, [tokensSearch]);
  const tokensQuery = useQuery({
    queryKey: [
      "user",
      "tokens",
      {
        page: tokensPage,
        pageSize: tokensPageSize,
        search: debouncedTokensSearch,
        sort: sanitizedTokensSortState,
        filters: tokenFilterParams,
      },
    ],
    queryFn: () =>
      listMyTokens({
          limit: tokensPageSize,
          offset: tokensPage * tokensPageSize,
          search: debouncedTokensSearch || undefined,
          sort: mapDataTableSortKey(
            sanitizedTokensSortState,
            USER_TOKENS_TABLE_CONFIG.sortKeyByColumn,
          ),
          order: sanitizedTokensSortState?.dir,
          scope: tokenFilterParams.scope,
          status: tokenFilterParams.status,
      }),
    placeholderData: (prev) => prev,
  });
  const tokenScopesQuery = useQuery({
    queryKey: ["user", "token-scope-catalog"],
    queryFn: listTokenScopes,
  });

  const [credDeleteTarget, setCredDeleteTarget] = useState<PersonalCredential | null>(null);

  const [tokenDeleteTarget, setTokenDeleteTarget] = useState<ApiToken | null>(null);
  const [passwordForm, setPasswordForm] = useState<PasswordForm>({
    currentPassword: "",
    newPassword: "",
    confirmPassword: "",
  });
  const [passwordErrors, setPasswordErrors] = useState<PasswordErrors>({});
  const [passwordSubmitError, setPasswordSubmitError] = useState(false);
  const passwordInputs = useRef<Partial<Record<PasswordField, HTMLInputElement | null>>>({});
  const passwordSubmitting = useRef(false);
  const passwordValidationStarted = useRef(false);
  const sectionParam = searchParams.get("section");
  const activeSection: UserSettingsSectionId = USER_SETTINGS_SECTION_IDS.includes(sectionParam as UserSettingsSectionId)
    ? sectionParam as UserSettingsSectionId : "profile";

  // Deep-link: `/user/settings?create=token` (e.g. from the MCP page) routes to
  // the token editor. The ref keeps it to one navigation per mount.
  const autoOpenTokenHandled = useRef(false);
  useEffect(() => {
    if (!autoOpenTokenHandled.current && searchParams.get("create") === "token") {
      autoOpenTokenHandled.current = true;
      void navigate(userSettingsEditorHref(USER_TOKEN_NEW_ROUTE, location), { replace: true });
    }
  }, [navigate, searchParams, location]);

  const credInvalidate = () => qc.invalidateQueries({ queryKey: ["user", "credentials"] });
  const tokenInvalidate = () => qc.invalidateQueries({ queryKey: ["user", "tokens"] }); // prefix-matches all token page queries


  const credDelete = useApiMutation({
    mutationFn: (id: string) => deleteMyCredential(id),
    onSuccess: () => {
      void credInvalidate();
      setCredDeleteTarget(null);
      notify("success", t("userSettings.credentialRemoved"));
    },
    onError: () => notify("error", t("userSettings.removeFailed")),
  });

  const credSetDefault = useApiMutation({
    mutationFn: (id: string) => setDefaultMyCredential(id),
    onSuccess: () => {
      void credInvalidate();
      notify("success", t("userSettings.credentialDefaultUpdated"));
    },
    onError: () => notify("error", t("userSettings.updateFailed")),
  });


  const tokenRevoke = useApiMutation({
    mutationFn: (id: string) => revokeMyToken(id),
    onSuccess: () => {
      void tokenInvalidate();
      setTokenDeleteTarget(null);
      notify("success", t("userSettings.tokenRevoked"));
    },
    onError: () => notify("error", t("userSettings.tokenRevokeFailed")),
  });

  const passwordChange = useApiMutation({
    mutationFn: (body: PasswordForm) =>
      changeMyPassword({
        currentPassword: body.currentPassword,
        newPassword: body.newPassword,
      }),
    onSuccess: () => {
      setPasswordForm({ currentPassword: "", newPassword: "", confirmPassword: "" });
      passwordSubmitting.current = false;
      passwordValidationStarted.current = false;
      setPasswordErrors({});
      setPasswordSubmitError(false);
      notify("success", t("userSettings.passwordChanged"));
    },
    onError: () => {
      passwordSubmitting.current = false;
      setPasswordSubmitError(true);
    },
  });

  const creds = useMemo(() => credsQuery.data ?? [], [credsQuery.data]);
  const credentialsFilterOptions = useMemo(
    () => ({
      protocol: PROTOCOLS.map((protocol) => ({ value: protocol.toLowerCase(), label: protocol })),
      isDefault: boolFilterOptions({
        trueLabel: t("userSettings.default"),
        falseLabel: t("userSettings.notDefault", "Not default"),
      }),
    }),
    [t],
  );
  const credentialColumns: Column<PersonalCredential>[] = [
      {
        key: "protocol",
        label: t("userSettings.protocol"),
        filterOptions: credentialsFilterOptions.protocol,
        getSortValue: (r: PersonalCredential) => r.protocol,
        render: (r: PersonalCredential) => (
          <span className="bg-muted px-2 py-0.5 rounded text-xs font-medium">
            {r.protocol}
          </span>
        ),
      },
      {
        key: "name",
        label: t("name"),
        getSortValue: (r: PersonalCredential) => r.name,
        render: (r: PersonalCredential) => (
          <span className="font-medium text-foreground">{r.name}</span>
        ),
      },
      {
        key: "isDefault",
        label: t("userSettings.default"),
        filterOptions: credentialsFilterOptions.isDefault,
        getSortValue: (r: PersonalCredential) => r.isDefault,
        render: (r: PersonalCredential) =>
          r.isDefault ? (
            <Star className="h-3 w-3 text-status-warning fill-status-warning" />
          ) : (
            <span className="text-muted-foreground">{t("userSettings.notDefault", "Not default")}</span>
          ),
      },
      {
        key: "actions",
        label: "",
        excludeFromSettings: true,
        render: (r: PersonalCredential) => (
          <RowActions label={`${t("actions")}: ${r.name}`} ref={(node) => { if (node) rowTriggers.current.set(r.id, node); else rowTriggers.current.delete(r.id); }}>
            <DropdownMenuItem asChild>
              <Link to={userSettingsEditorHref(userCredentialEditHref(r.id), location)}>
                <Pencil className="h-3 w-3" />
                {t("edit")}
              </Link>
            </DropdownMenuItem>
            {!r.isDefault && <DropdownMenuItem onSelect={() => credSetDefault.mutate(r.id)}><Star className="size-4" />{t("userSettings.setDefault")}</DropdownMenuItem>}
            <DropdownMenuItem variant="destructive" onSelect={() => { returnFocus.current = rowTriggers.current.get(r.id) ?? null; setCredDeleteTarget(r); }}><Trash2 className="size-4" />{t("delete")}</DropdownMenuItem>
          </RowActions>
        ),
      },
    ];
  const tokens = useMemo(() => tokensQuery.data?.items ?? [], [tokensQuery.data]);
  const tokensTotal = tokensQuery.data?.total ?? 0;
  const observedTokenScopeOptions = useMemo(() => {
    const values = new Set<string>();
    for (const token of tokens) {
      for (const scope of token.scopes.split(/[,\s]+/).filter(Boolean)) {
        const entry = tokenScopeCatalogEntry(scope);
        if (entry) values.add(entry.value);
      }
    }
    for (const scope of sanitizedTokensColumnFilters.scopes ?? []) {
      const entry = tokenScopeCatalogEntry(scope);
      if (entry) values.add(entry.value);
    }

    return Array.from(values)
      .map((value) => tokenScopeCatalogEntry(value))
      .filter((entry): entry is NonNullable<typeof entry> => entry !== null);
  }, [sanitizedTokensColumnFilters.scopes, tokens]);
  const tokenScopeOptions = useMemo(
    () => mergeTokenScopeOptions(tokenScopesQuery.data ?? [], observedTokenScopeOptions),
    [observedTokenScopeOptions, tokenScopesQuery.data],
  );
  const tokenScopeFilterOptions = useMemo(
    () =>
      tokenScopeOptions.map((scope) => ({
        value: scope.value,
        label: scope.label,
      })),
    [tokenScopeOptions],
  );
  const tokenStatusFilterOptions = useMemo(
    () => [
      { value: "active", label: t("userSettings.tokenStatus.active", "Active") },
      { value: "revoked", label: t("userSettings.tokenStatus.revoked", "Revoked") },
    ],
    [t],
  );
  const tokenColumns: Column<ApiToken>[] = [
      {
        key: "name",
        label: t("name"),
        sortable: true,
        render: (r: ApiToken) => <span className="font-medium text-foreground">{r.name}</span>,
      },
      {
        key: "scopes",
        label: t("userSettings.scopes"),
        sortable: true,
        filterOptions: tokenScopeFilterOptions,
        getSortValue: (r: ApiToken) => r.scopes,
        render: (r: ApiToken) => (
          <div className="flex gap-1 flex-wrap">
            {r.scopes
              .split(/[,\s]+/)
              .filter(Boolean)
              .map((s) => (
                <span
                  key={s}
                  className="bg-muted px-1.5 py-0 rounded text-xs font-mono"
                >
                  {s}
                </span>
              ))}
          </div>
        ),
      },
      {
        key: "status",
        label: t("status"),
        sortable: true,
        filterOptions: tokenStatusFilterOptions,
        render: (r: ApiToken) => (
          <StatusChip
            status={r.status === "active" ? "positive" : "negative"}
            label={r.status === "active" ? t("userSettings.tokenStatus.active", "Active") : t("userSettings.tokenStatus.revoked", "Revoked")}
          />
        ),
      },
      { key: "createdAt", label: t("userSettings.created"), sortable: true },
      { key: "expiresAt", label: t("userSettings.expires"), sortable: true },
      {
        key: "actions",
        label: "",
        excludeFromSettings: true,
        render: (r: ApiToken) => (
          <RowActions label={`${t("actions")}: ${r.name}`} ref={(node) => { if (node) rowTriggers.current.set(`token:${r.id}`, node); else rowTriggers.current.delete(`token:${r.id}`); }}>
            <DropdownMenuItem variant="destructive" onSelect={() => { returnFocus.current = rowTriggers.current.get(`token:${r.id}`) ?? null; setTokenDeleteTarget(r); }}>
              <Trash2 className="size-4" />{t("revoke")}
            </DropdownMenuItem>
          </RowActions>
        ),
      },
    ];
  const currentUser = meQuery.data;
  const settingsSections = useMemo<UserSettingsSection[]>(
    () => [
      {
        id: "profile",
        title: t("userSettings.profileSection"),
        description: t("userSettings.sectionDescriptions.profile"),
        Icon: Shield,
        badge: currentUser?.authSource ?? t("userSettings.local"),
      },
      {
        id: "interface",
        title: t("userSettings.interfaceSection"),
        description: t("userSettings.sectionDescriptions.interface"),
        // count: theme, language, ui scale, config-context format
        Icon: Palette,
        badge: t("settingsCatalog.sectionSettings", { count: 4 }),
      },
      {
        id: "connections",
        title: t("userSettings.connectionPreferences"),
        description: t("userSettings.sectionDescriptions.connections"),
        Icon: MonitorIcon,
      },
      {
        id: "credentials",
        title: t("userSettings.personalCredentialSets"),
        description: t("userSettings.sectionDescriptions.credentials"),
        Icon: Key,
        badge: t("userSettings.credentialCount", { count: creds.length }),
      },
      {
        id: "tokens",
        title: t("userSettings.apiTokens"),
        description: t("userSettings.sectionDescriptions.tokens"),
        Icon: KeyRound,
        badge: t("userSettings.tokenCount", { count: tokensTotal }),
      },
    ],
    [creds.length, currentUser?.authSource, t, tokensTotal],
  );
  const selectedSettingsSection =
    settingsSections.find((section) => section.id === activeSection) ?? settingsSections[0]!;

  const handleCredentialsSortChange = (next: DataTableSortState | null) => {
    setCredentialsSortState(next);
  };

  const handleCredentialsColumnFilterChange = (key: string, values: string[]) => {
    setCredentialsColumnFilters((current) => updateDataTableColumnFilters(current, key, values));
  };

  const handleTokensSortChange = (next: DataTableSortState | null) => {
    setTokensSortState(next);
    setTokensPage(0);
  };

  const handleTokensColumnFilterChange = (key: string, values: string[]) => {
    setTokensColumnFilters((current) => updateDataTableColumnFilters(current, key, values));
    setTokensPage(0);
  };


  const validatePassword = (form: PasswordForm): PasswordErrors => ({
    ...(!form.currentPassword ? { currentPassword: "currentPasswordRequired" } : {}),
    ...(!form.newPassword ? { newPassword: "newPasswordRequired" }
      : form.newPassword.length < 8 ? { newPassword: "passwordTooShort" } : {}),
    ...(!form.confirmPassword ? { confirmPassword: "confirmPasswordRequired" }
      : form.newPassword !== form.confirmPassword ? { confirmPassword: "passwordsDontMatch" } : {}),
  });

  const updatePasswordField = (field: PasswordField, value: string) => {
    const next = { ...passwordForm, [field]: value };
    setPasswordForm(next);
    setPasswordSubmitError(false);
    const validated = validatePassword(next);
    setPasswordErrors(current => {
      const updated = { ...current };
      for (const key of PASSWORD_FIELDS) {
        if (passwordValidationStarted.current && (key === field || (field === "newPassword" && key === "confirmPassword"))) {
          if (validated[key]) updated[key] = validated[key];
          else delete updated[key];
        }
      }
      return updated;
    });
  };

  const handlePasswordChange = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (passwordSubmitting.current || passwordChange.isPending) return;
    passwordValidationStarted.current = true;
    const errors = validatePassword(passwordForm);
    setPasswordErrors(errors);
    setPasswordSubmitError(false);
    const firstInvalid = PASSWORD_FIELDS.find(field => errors[field]);
    if (firstInvalid) {
      passwordInputs.current[firstInvalid]?.focus();
      return;
    }
    passwordSubmitting.current = true;
    passwordChange.mutate(passwordForm);
  };

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <PageChrome
        title={t("userSettings.title")}
        description={t("userSettings.description")}
      />

      <div className="flex min-w-0 flex-col gap-4">
        <SectionLinks
          activeValue={activeSection}
          label={t("userSettings.sections")}
          items={settingsSections.map(({ id, title, Icon, badge }) => {
            const params = new URLSearchParams(searchParams);
            if (id === "profile") params.delete("section");
            else params.set("section", id);
            return { value: id, label: title, icon: Icon, badge,
              to: { pathname: location.pathname, search: params.toString(), hash: location.hash } };
          })}
        />

        <div className="min-w-0 space-y-4">
          {activeSection === "profile" ? (
            <>
              <UserSettingsSectionHeader section={selectedSettingsSection} />
              <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
                <SurfaceCard className="overflow-hidden">
                  <SurfaceCardHeader headingLevel={3} title={<span className="flex items-center gap-2"><Shield className="size-4 text-primary" aria-hidden="true" />{t("userSettings.currentUser")}</span>} />
                  <div className="space-y-3 p-4">
                    {meQuery.isPending ? (
                      <div className="flex items-center justify-center py-4 text-sm text-muted-foreground">
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />{" "}
                        {t("userSettings.loadingUser")}
                      </div>
                    ) : currentUser ? (
                      <>
                        <div className="flex items-center gap-3">
                          <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-accent">
                            <span className="text-lg font-bold text-primary">
                              {currentUser.username.charAt(0).toUpperCase()}
                            </span>
                          </div>
                          <div className="min-w-0">
                            <p className="truncate text-sm font-medium text-foreground">
                              {currentUser.username}
                            </p>
                            <p className="truncate text-sm text-muted-foreground">
                              {currentUser.email}
                            </p>
                          </div>
                        </div>
                        <div className="flex flex-wrap gap-1.5">
                          <span className="bg-muted rounded px-2 py-0.5 text-xs">
                            {currentUser.authSource ?? t("userSettings.local")}
                          </span>
                          <StatusChip
                            status={currentUser.status === "active" ? "positive" : "neutral"}
                            label={
                              currentUser.status
                                ? t(`statuses.${currentUser.status}`, currentUser.status)
                                : ""
                            }
                          />
                        </div>
                      </>
                    ) : null}
                  </div>
                </SurfaceCard>

                <SurfaceCard className="overflow-hidden lg:col-span-2">
                  <SurfaceCardHeader headingLevel={3} title={<span className="flex items-center gap-2"><KeyRound className="size-4 text-primary" aria-hidden="true" />{t("userSettings.loginPassword")}</span>} />
                  <div className="space-y-3 p-4">
                    {meQuery.isPending ? (
                      <div className="flex items-center py-4 text-sm text-muted-foreground">
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />{" "}
                        {t("userSettings.loadingUser")}
                      </div>
                    ) : currentUser?.authSource === "local" ? (
                      <form noValidate onSubmit={handlePasswordChange} className="flex flex-col gap-3" aria-busy={passwordChange.isPending}>
                        {PASSWORD_FIELDS.map(field => {
                          const id = `account-${field.replace(/Password$/, "-password")}`;
                          const error = passwordErrors[field];
                          return (
                            <FormField key={field} htmlFor={id} label={t(`userSettings.${field}`)} required
                              error={error ? t(`userSettings.${error}`) : undefined}>
                              <FormFieldControl>
                              <Input id={id} type="password"
                                ref={input => { passwordInputs.current[field] = input; }}
                                autoComplete={field === "currentPassword" ? "current-password" : "new-password"}
                                value={passwordForm[field]} disabled={passwordChange.isPending}
                                onChange={event => updatePasswordField(field, event.target.value)} />
                              </FormFieldControl>
                            </FormField>
                          );
                        })}
                        {passwordSubmitError ? <p role="alert" className="text-sm text-destructive">{t("userSettings.passwordChangeFailed")}</p> : null}
                        {passwordChange.isPending ? <p role="status" className="sr-only">{t("userSettings.passwordSaving")}</p> : null}
                        <Button type="submit" disabled={passwordChange.isPending}>
                          {t(passwordChange.isPending ? "userSettings.passwordSaving" : "userSettings.changePassword")}
                        </Button>
                      </form>
                    ) : (
                      <p className="text-sm text-muted-foreground">
                        {t("userSettings.externalPasswordHint")}
                      </p>
                    )}
                  </div>
                </SurfaceCard>

                <SurfaceCard className="overflow-hidden lg:col-span-3">
                  <SurfaceCardHeader headingLevel={3} title={<span className="flex items-center gap-2"><Lock className="size-4 text-primary" aria-hidden="true" />{t("userSettings.session")}</span>} />
                  <div className="space-y-2 p-4">
                    {currentUser?.expiresAt ? (
                      <p className="text-sm text-muted-foreground">
                        {t("userSettings.sessionExpires", {
                          date: formatSessionExpiry(currentUser.expiresAt, i18n.language),
                        })}
                      </p>
                    ) : (
                      <p className="text-sm text-muted-foreground">
                        {t("userSettings.sessionActive")}
                      </p>
                    )}
                  </div>
                </SurfaceCard>
              </div>
            </>
          ) : null}

          {activeSection === "interface" ? (
            <>
              <UserSettingsSectionHeader section={selectedSettingsSection} />
              <SettingsCatalog
                mode="user"
                keys={["app.theme", "app.language", "ui.scale", CONFIG_CONTEXT_FORMAT_SETTING_KEY]}
                showHeader={false}
                showSectionNav={false}
              />
            </>
          ) : null}

          {activeSection === "connections" ? (
            <>
              <UserSettingsSectionHeader section={selectedSettingsSection} />
              <SettingsCatalog
                mode="user"
                keys={["connections.open_in_new_tab"]}
                showHeader={false}
                showSectionNav={false}
              />
            </>
          ) : null}

          {activeSection === "credentials" ? (
            <>
              <UserSettingsSectionHeader section={selectedSettingsSection} />
              <PageActions>
                  <Button asChild variant="outline" className="gap-1">
                    <Link to={userSettingsEditorHref(USER_CREDENTIAL_NEW_ROUTE, location)}>
                      <Plus className="h-3 w-3" /> {t("userSettings.addCredential")}
                    </Link>
                  </Button>
              </PageActions>
                <DataTable
                  tableId="userSettings.credentials"
                  columns={credentialColumns}
                  data={creds}
                  getRowKey={(r) => r.id}
                  loading={credsQuery.isPending}
                  error={credsQuery.isError ? credsQuery.error : undefined}
                  onRetry={() => credsQuery.refetch()}
                  sortState={sanitizedCredentialsSortState}
                  onSortChange={handleCredentialsSortChange}
                  columnFilters={sanitizedCredentialsColumnFilters}
                  onColumnFilterChange={handleCredentialsColumnFilterChange}
                  emptyState={
                    credsQuery.isPending ? (
                      <div className="flex items-center justify-center py-8 text-sm text-muted-foreground">
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />{" "}
                        {t("userSettings.loadingCredentials")}
                      </div>
                    ) : (
                      <EmptyState
                        icon={Key}
                        title={t("userSettings.noCredentialSets")}
                        description={t("userSettings.noCredentialSetsDescription")}
                      />
                    )
                  }
                />
            </>
          ) : null}

          {activeSection === "tokens" ? (
            <>
              <UserSettingsSectionHeader section={selectedSettingsSection} />
              <PageActions>
                  <Button asChild variant="outline" className="gap-1">
                    <Link to={userSettingsEditorHref(USER_TOKEN_NEW_ROUTE, location)}>
                      <Plus className="h-3 w-3" /> {t("userSettings.createToken")}
                    </Link>
                  </Button>
              </PageActions>
                <DataTable
                  tableId="userSettings.tokens"
                  columns={tokenColumns}
                  data={tokens}
                  getRowKey={(r) => r.id}
                  loading={tokensQuery.isPending}
                  error={tokensQuery.isError ? tokensQuery.error : undefined}
                  onRetry={() => tokensQuery.refetch()}
                  sortState={sanitizedTokensSortState}
                  onSortChange={handleTokensSortChange}
                  columnFilters={sanitizedTokensColumnFilters}
                  onColumnFilterChange={handleTokensColumnFilterChange}
                  pagination={{
                    page: tokensPage,
                    pageSize: tokensPageSize,
                    total: tokensTotal,
                    onPageChange: setTokensPage,
                    onPageSizeChange: (s) => {
                      setTokensPageSize(s);
                      setTokensPage(0);
                    },
                  }}
                  toolbar={
                    <SearchInput
                      value={tokensSearch}
                      onValueChange={setTokensSearch}
                      placeholder={t("userSettings.tokenSearchPlaceholder", "Search tokens")}
                      containerClassName="max-w-xs flex-1"
                    />
                  }
                  emptyState={
                    tokensQuery.isPending ? (
                      <div className="flex items-center justify-center py-8 text-sm text-muted-foreground">
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />{" "}
                        {t("userSettings.loadingTokens")}
                      </div>
                    ) : (
                      <EmptyState
                        icon={Key}
                        title={t("userSettings.noApiTokens")}
                        description={t("userSettings.noApiTokensDescription")}
                      />
                    )
                  }
                />
            </>
          ) : null}
        </div>
      </div>


      <ConfirmDeleteDialog
        open={!!credDeleteTarget || !!tokenDeleteTarget}
        onOpenChange={(open) => {
          if (!open) { setCredDeleteTarget(null); setTokenDeleteTarget(null); }
        }}
        contentClassName="max-h-[90dvh] overflow-y-auto bg-card"
        onCloseAutoFocus={(event) => { event.preventDefault(); returnFocus.current?.focus(); }}
        title={t(tokenDeleteTarget ? "userSettings.revokeToken" : "userSettings.removeCredential")}
        description={
          <Trans
            i18nKey={tokenDeleteTarget ? "userSettings.revokeTokenDescription" : "userSettings.removeCredentialDescription"}
            values={{ name: tokenDeleteTarget?.name ?? credDeleteTarget?.name ?? "" }}
            components={{ 1: <strong /> }}
          />
        }
        cancelLabel={t("cancel")}
        confirmLabel={t(tokenDeleteTarget ? "revoke" : "remove")}
        confirmProps={{
          onClick: () => {
            if (tokenDeleteTarget) tokenRevoke.mutate(tokenDeleteTarget.id);
            else if (credDeleteTarget) credDelete.mutate(credDeleteTarget.id);
          },
        }}
      />
    </div>
  );
};

export default UserSettings;
