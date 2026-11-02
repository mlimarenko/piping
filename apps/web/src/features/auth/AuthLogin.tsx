import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Lock, Loader2, AlertCircle, Eye, EyeOff } from "lucide-react";
import { buildSsoStartUrl, getAuthProviders, login, safeLoginReturnTo } from "@/features/auth/api";
import {
  authErrorMessageFromApiError,
  authErrorMessageFromCode,
} from "@/features/auth/errors";
import type { ApiError } from "@/shared/api/client";

const AuthLogin = () => {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const returnTo = safeLoginReturnTo(searchParams.get("redirect_to"));
  const { t } = useTranslation();
  const [identifier, setIdentifier] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [formError, setFormError] = useState("");
  const [dismissedQueryErrorCode, setDismissedQueryErrorCode] = useState<
    string | null
  >(null);
  const queryErrorCode = searchParams.get("error");
  const queryError =
    authErrorMessageFromCode(t, queryErrorCode) ??
    (queryErrorCode ? t("auth.errors.ssoFailed") : "");
  const queryErrorDismissed =
    queryErrorCode !== null && dismissedQueryErrorCode === queryErrorCode;
  const error = formError || (queryErrorDismissed ? "" : queryError);
  const dismissQueryError = () => setDismissedQueryErrorCode(queryErrorCode);

  const providers = useQuery({
    queryKey: ["auth", "providers"],
    queryFn: getAuthProviders,
    retry: false,
  });

  const loginMutation = useApiMutation({
    mutationFn: login,
    onSuccess: () => navigate(returnTo, { replace: true }),
    onError: (err: ApiError) => {
      dismissQueryError();
      setFormError(
        authErrorMessageFromApiError(t, err) ?? t("auth.signInFailed"),
      );
    },
  });

  const handleIdentifierChange = (value: string) => {
    if (loginMutation.isPending) return;
    setIdentifier(value);
    setFormError("");
    dismissQueryError();
  };

  const handlePasswordChange = (value: string) => {
    if (loginMutation.isPending) return;
    setPassword(value);
    setFormError("");
    dismissQueryError();
  };

  const handleLogin = (e: React.FormEvent) => {
    e.preventDefault();
    if (loginMutation.isPending) return;
    if (!identifier.trim()) {
      dismissQueryError();
      setFormError(t("auth.identifierRequired"));
      return;
    }
    if (!password) {
      dismissQueryError();
      setFormError(t("auth.passwordRequired"));
      return;
    }
    loginMutation.mutate({ identifier: identifier.trim(), password });
  };

  const handleSso = () => {
    if (loginMutation.isPending) return;
    setFormError("");
    dismissQueryError();
    window.location.href = buildSsoStartUrl(returnTo);
  };

  const loading = loginMutation.isPending;
  const localEnabled = providers.data?.localEnabled ?? true;
  const oidcEnabled = providers.data?.oidcEnabled ?? false;

  return (
    <main className="flex min-h-full items-center justify-center bg-background p-4">
      <SurfaceCard className="w-full max-w-sm p-6">
        <div className="mb-6 flex items-center gap-3">
          <div className="surface-chip rounded-lg p-2">
            <Lock className="h-4 w-4 text-primary" />
          </div>
          <h1 className="text-xl font-bold text-foreground">{t("auth.brand")}</h1>
        </div>

        {error && (
          <div id="login-error" role="alert" className="mb-4 flex items-start gap-2 rounded-md border border-destructive/25 bg-destructive/10 p-3 text-sm text-destructive">
            <AlertCircle className="h-3.5 w-3.5" />
            {error}
          </div>
        )}

        {localEnabled && (
          <form onSubmit={handleLogin} className="flex flex-col gap-4">
            <FormField htmlFor="login-identifier" label={t("auth.identifier")}>
              <FormFieldControl>
              <Input
                id="login-identifier"
                disabled={loading}
                value={identifier}
                onChange={(e) => handleIdentifierChange(e.target.value)}
                placeholder={t("auth.identifierPlaceholder")}
                aria-required="true"
                aria-describedby={error ? "login-error" : undefined}
                autoComplete="username"
              />
              </FormFieldControl>
            </FormField>
            <FormField htmlFor="login-password" label={t("auth.password")}>
              <div className="relative">
                <FormFieldControl>
                <Input
                  id="login-password"
                  disabled={loading}
                  type={showPassword ? "text" : "password"}
                  value={password}
                  onChange={(e) => handlePasswordChange(e.target.value)}
                  placeholder={t("auth.passwordPlaceholder")}
                  className="pr-10"
                  aria-required="true"
                  aria-describedby={error ? "login-error" : undefined}
                  autoComplete="current-password"
                />
                </FormFieldControl>
                <button
                  type="button"
                  disabled={loading}
                  onClick={() => setShowPassword((v) => !v)}
                  aria-label={
                    showPassword
                      ? t("auth.hidePassword", "Hide password")
                      : t("auth.showPassword", "Show password")
                  }
                  aria-pressed={showPassword}
                  className="absolute right-1 top-1/2 inline-flex size-8 -translate-y-1/2 items-center justify-center rounded-sm text-muted-foreground hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  {showPassword ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
                </button>
              </div>
            </FormField>
            <Button type="submit" className="w-full" disabled={loading}>
              {loading ? <Loader2 className="h-4 w-4 animate-spin" /> : t("auth.signIn")}
            </Button>
          </form>
        )}

        {oidcEnabled && (
          <div className={localEnabled ? "mt-4 pt-3 border-t border-border/40" : ""}>
            <Button
              variant="outline"
               className="w-full"
              onClick={handleSso}
              disabled={loading}
            >
              {t("auth.continueWithSso")}
            </Button>
          </div>
        )}
      </SurfaceCard>
    </main>
  );
};

export default AuthLogin;
