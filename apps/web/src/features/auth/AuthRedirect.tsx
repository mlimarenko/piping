import { useEffect } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { Loader2, AlertCircle } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { authErrorMessageFromCode } from "@/features/auth/errors";

const AuthRedirect = () => {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const { t } = useTranslation();
  const errorCode = searchParams.get("error");
  const error =
    authErrorMessageFromCode(t, errorCode) ??
    (errorCode ? t("auth.errors.ssoFailed") : null);

  useEffect(() => {
    if (errorCode) return undefined;
    const timer = setTimeout(() => {
      void navigate("/connections");
    }, 1500);
    return () => clearTimeout(timer);
  }, [errorCode, navigate]);

  const handleBackToLogin = () => {
    const params = new URLSearchParams();
    if (errorCode) params.set("error", errorCode);
    const query = params.toString();
    void navigate(query ? `/auth/login?${query}` : "/auth/login");
  };

  if (error) {
    return (
      <main className="flex min-h-[var(--app-vh)] items-center justify-center p-4">
        <div className="flex w-full max-w-sm flex-col items-center gap-4 rounded-xl border border-border bg-card p-6 text-center shadow-md">
          <AlertCircle className="size-8 text-destructive" aria-hidden="true" />
          <h1 className="text-xl font-semibold tracking-tight">{t("auth.signInFailed")}</h1>
          <p role="alert" className="text-sm text-foreground">{error}</p>
          <Button onClick={handleBackToLogin}>
            {t("auth.backToLogin")}
          </Button>
        </div>
      </main>
    );
  }

  return (
    <div className="flex min-h-[var(--app-vh)] items-center justify-center">
      <div className="text-center">
        <Loader2 className="h-6 w-6 animate-spin text-primary mx-auto mb-3" />
        <p className="text-sm text-muted-foreground">{t("auth.processing")}</p>
      </div>
    </div>
  );
};

export default AuthRedirect;
