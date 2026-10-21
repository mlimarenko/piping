import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { AlertCircle, Loader2 } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import { Button } from "@/shared/ui-primitives/button";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { Input } from "@/shared/ui-primitives/input";
import SecretField from "@/shared/ui-shared/SecretField";
import { cn } from "@/shared/lib/utils";
import { me } from "@/features/auth/api";
import type { AdHocCredentials } from "@/shared/api/types";
import {
  LAUNCH_PROTOCOL_COLOR,
  LAUNCH_PROTOCOL_LABEL,
  type SessionLaunchProtocol,
} from "./launchPresentation";

// ── Public contract ───────────────────────────────────────────────────────────

/** SSH authentication method. Mirrors the backend's mutually-exclusive secrets. */
export type SshAuthMethod = "password" | "private_key";

/**
 * Context that opens the dialog. The dialog completes an existing launch account
 * that still needs a secret ("fill-in"): the account's username is fixed and
 * read-only, and only the missing secret is collected. The entered secret is
 * threaded onto the selected account at connect time (backend merges it).
 *
 * A fully ad-hoc "fresh" connect (no account, all fields editable) is NOT offered
 * here: the connect path requires the host to already have connection-config for
 * the protocol, which a credential-less account always implies but a bare host
 * does not. See the feature notes for the backend gap.
 */
export interface AdHocCredentialsContext {
  protocol: SessionLaunchProtocol;
  host: { id: string; name: string };
  /** The launch account being completed. */
  account: {
    entryId: string;
    name: string;
    username?: string;
  };
}

export interface AdHocCredentialsSubmit {
  protocol: SessionLaunchProtocol;
  host: { id: string; name: string };
  /** The selected account; threaded through to the session open. */
  entryId: string;
  /** The account's remote login, for the session-tab title (not re-sent inline). */
  username?: string;
  credentials: AdHocCredentials;
  /** When true, also persist these as the caller's per-protocol default. */
  makeDefault: boolean;
}

interface AdHocCredentialsDialogProps {
  context: AdHocCredentialsContext | null;
  onOpenChange: (open: boolean) => void;
  /**
   * Performs the connect (and optional default-save). Rejects with an error
   * whose message is shown in the backend-error banner; resolves on success,
   * after which the dialog closes itself.
   */
  onSubmit: (submit: AdHocCredentialsSubmit) => Promise<void>;
}

// ── Form state ────────────────────────────────────────────────────────────────

interface FormState {
  username: string;
  password: string;
  privateKey: string;
  sshMethod: SshAuthMethod;
  makeDefault: boolean;
}

type FieldError = "password" | "privateKey";

/**
 * Validate the form against the backend credential contract for the protocol.
 * SSH requires exactly one of {password, private_key}; RDP requires a password;
 * VNC has no required field. The account already carries the username, so it is
 * not validated here. Mirrors, does not exceed, the server-side rules.
 */
function validate(
  protocol: SessionLaunchProtocol,
  form: FormState,
): Partial<Record<FieldError, true>> {
  const errors: Partial<Record<FieldError, true>> = {};
  if (protocol === "ssh") {
    if (form.sshMethod === "password" && !form.password.trim()) errors.password = true;
    if (form.sshMethod === "private_key" && !form.privateKey.trim()) errors.privateKey = true;
  } else if (protocol === "rdp") {
    if (!form.password.trim()) errors.password = true;
  }
  // VNC: password is optional — no required-field errors.
  return errors;
}

/** Build the wire credentials object for the protocol from the form state. */
function buildCredentials(
  protocol: SessionLaunchProtocol,
  form: FormState,
): AdHocCredentials {
  const username = form.username.trim() || undefined;
  if (protocol === "ssh") {
    return form.sshMethod === "password"
      ? { username, password: form.password }
      : { username, private_key: form.privateKey };
  }
  if (protocol === "rdp") {
    // Domain is not collected in this dialog; only username + password.
    return { username, password: form.password || undefined };
  }
  // VNC: only password; no username field.
  return { password: form.password || undefined };
}

// ── Outer dialog ───────────────────────────────────────────────────────────────
//
// Owns only the Dialog shell. The form body is a separate component, remounted
// per `context` (via `key`) so its state always initializes fresh — no
// reset-in-effect, no cascading renders.

const AdHocCredentialsDialog = ({
  context,
  onOpenChange,
  onSubmit,
}: AdHocCredentialsDialogProps) => {
  return (
    <Dialog open={context !== null} onOpenChange={onOpenChange}>
      {context && (
        <AdHocCredentialsForm
          key={`${context.host.id}:${context.protocol}:${context.account.entryId}`}
          context={context}
          onOpenChange={onOpenChange}
          onSubmit={onSubmit}
        />
      )}
    </Dialog>
  );
};

export default AdHocCredentialsDialog;

// ── Inner form ───────────────────────────────────────────────────────────────

interface AdHocCredentialsFormProps {
  context: AdHocCredentialsContext;
  onOpenChange: (open: boolean) => void;
  onSubmit: (submit: AdHocCredentialsSubmit) => Promise<void>;
  onCloseAutoFocus?: (event: Event) => void;
}

export const AdHocCredentialsForm = ({
  context,
  onOpenChange,
  onSubmit,
  onCloseAutoFocus,
}: AdHocCredentialsFormProps) => {
  const { t } = useTranslation();
  const { protocol, account } = context;
  const protocolLabel = LAUNCH_PROTOCOL_LABEL[protocol];

  // Fetch the session user as username fallback (cache hit — no network round-trip
  // if the app shell already fetched it). Only used for the initial state below.
  const meQuery = useQuery({ queryKey: ["auth", "me"], queryFn: me, staleTime: Infinity });

  const [form, setForm] = useState<FormState>(() => ({
    // Priority: account carries a literal username → else current session user.
    username: account.username ?? meQuery.data?.username ?? "",
    password: "",
    privateKey: "",
    sshMethod: "password",
    makeDefault: false,
  }));
  const [showErrors, setShowErrors] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [backendError, setBackendError] = useState<string | null>(null);

  const errors = useMemo(() => validate(protocol, form), [protocol, form]);

  const update = <K extends keyof FormState>(key: K, value: FormState[K]) => {
    setForm((prev) => ({ ...prev, [key]: value }));
    if (backendError) setBackendError(null);
  };

  const handleSubmit = async () => {
    const validationErrors = validate(protocol, form);
    if (Object.keys(validationErrors).length > 0) {
      setShowErrors(true);
      return;
    }
    setSubmitting(true);
    setBackendError(null);
    try {
      await onSubmit({
        protocol,
        host: context.host,
        entryId: account.entryId,
        // Use the (potentially overridden) username from the form for the tab title.
        username: form.username.trim() || account.username,
        credentials: buildCredentials(protocol, form),
        makeDefault: form.makeDefault,
      });
      onOpenChange(false);
    } catch (err) {
      setBackendError(err instanceof Error ? err.message : String(err));
      setSubmitting(false);
    }
  };

  const title =
    protocol === "ssh"
      ? t("connections.adHocCredentials.titleFillInKey")
      : t("connections.adHocCredentials.titleFillIn");

  return (
    <DialogContent
      onCloseAutoFocus={onCloseAutoFocus}
      className="max-w-md"
      description={t("connections.adHocCredentials.subtitleFillIn")}
      onInteractOutside={(e) => {
        if (submitting) e.preventDefault();
      }}
      onEscapeKeyDown={(e) => {
        if (submitting) e.preventDefault();
      }}
    >
      <DialogHeader>
        <DialogTitle className="flex items-center gap-2">
          <span className={cn("text-xs font-semibold uppercase", LAUNCH_PROTOCOL_COLOR[protocol])}>
            {protocolLabel}
          </span>
          <span>{title}</span>
        </DialogTitle>
      </DialogHeader>

      {/* Context banner: host + account name. */}
      <div className="rounded-md border border-border/40 bg-muted px-3 py-2 text-xs text-muted-foreground">
        <div className="font-medium text-foreground">{context.host.name}</div>
        <div className="mt-0.5">
          {t("connections.adHocCredentials.contextAccountName", { name: account.name })}
        </div>
      </div>

      {/* Backend-error banner — fields stay active; button becomes "Retry". */}
      {backendError && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
        >
          <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <div>
            <div className="font-medium">{t("connections.adHocCredentials.backendError")}</div>
            <div className="mt-0.5 break-words opacity-90">{backendError}</div>
          </div>
        </div>
      )}

      {/* Form body — dims + disables interaction while submitting. */}
      <div className={cn("space-y-3", submitting && "pointer-events-none opacity-50")}>
        {/* Editable username — SSH and RDP only (VNC has no username concept).
            Prefilled from the account's stored username, falling back to the
            current session user. The user may override it; the trimmed value is
            sent as credentials.username and merged by the backend. */}
        {(protocol === "ssh" || protocol === "rdp") && (
          <div>
            <label className="mb-1 block text-xs font-medium text-muted-foreground">
              {t("connectionConfig.fields.username")}
            </label>
            <Input
              value={form.username}
              onChange={(e) => update("username", e.target.value)}
              placeholder={t("connectionConfig.fields.usernamePlaceholder")}
              autoComplete="username"
              className="h-8 text-sm"
            />
          </div>
        )}

        {/* SSH method toggle (pill buttons). The account fixes the username but
            launch-options carries no auth-method signal, so the user picks
            whether they are providing a password or a private key. */}
        {protocol === "ssh" && (
          <div>
            <label className="mb-1 block text-xs font-medium text-muted-foreground">
              {t("connections.adHocCredentials.authMethod")}
            </label>
            <div className="grid grid-cols-2 gap-1">
              {(["password", "private_key"] as const).map((method) => {
                const active = form.sshMethod === method;
                return (
                  <button
                    key={method}
                    type="button"
                    className={cn(
                      "flex h-9 items-center justify-center rounded-md border px-1.5 text-sm font-semibold uppercase transition-colors",
                      active
                        ? "border-primary/50 bg-primary/15 text-primary"
                        : "border-input/30 text-muted-foreground hover:border-input/60 hover:text-foreground",
                    )}
                    onClick={() => update("sshMethod", method)}
                    aria-pressed={active}
                  >
                    {method === "password"
                      ? t("connections.adHocCredentials.authPassword")
                      : t("connections.adHocCredentials.authPrivateKey")}
                  </button>
                );
              })}
            </div>
          </div>
        )}

        {/* Secret field: SSH password OR private key (per toggle); RDP/VNC
            password. Errors render as a red border + inline hint below. */}
        {protocol === "ssh" && form.sshMethod === "private_key" ? (
          <div>
            <SecretField
              label={t("connectionConfig.fields.privateKey")}
              value={form.privateKey || undefined}
              multiline
              canReveal
              loading={false}
              onChange={(next) => update("privateKey", next ?? "")}
              placeholder={t("connectionConfig.fields.privateKeyPlaceholder")}
            />
            {showErrors && errors.privateKey && (
              <p className="mt-1 text-xs text-destructive">
                {t("connections.adHocCredentials.validation.privateKeyRequired")}
              </p>
            )}
          </div>
        ) : (
          <div>
            <SecretField
              label={t("connectionConfig.fields.password")}
              value={form.password || undefined}
              canReveal
              loading={false}
              onChange={(next) => update("password", next ?? "")}
              placeholder={t("connectionConfig.fields.passwordPlaceholder")}
            />
            {showErrors && errors.password && (
              <p className="mt-1 text-xs text-destructive">
                {t("connections.adHocCredentials.validation.passwordRequired")}
              </p>
            )}
          </div>
        )}

        {/* Optional: save as the caller's per-protocol default. */}
        <label className="flex items-center gap-2 pt-1 text-xs text-muted-foreground">
          <Checkbox
            checked={form.makeDefault}
            onCheckedChange={(checked) => update("makeDefault", checked === true)}
          />
          {t("connections.adHocCredentials.makeDefault", { protocol: protocolLabel })}
        </label>
      </div>

      {/* Footer actions. */}
      <div className="flex justify-end gap-2 pt-1">
        <Button
          variant="outline"
          size="sm"
          disabled={submitting}
          onClick={() => onOpenChange(false)}
        >
          {t("cancel")}
        </Button>
        <Button size="sm" disabled={submitting} onClick={handleSubmit}>
          {submitting && <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />}
          {submitting
            ? t("connections.adHocCredentials.connectingBtn")
            : backendError
              ? t("connections.adHocCredentials.retryBtn")
              : t("connections.adHocCredentials.connectBtn")}
        </Button>
      </div>
    </DialogContent>
  );
};
