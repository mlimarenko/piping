import { AlertTriangle, Info, RotateCw, WifiOff } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ConnectionFailure } from "@/features/connections/connectionFailure";
import { Button } from "@/shared/ui-primitives/button";

interface ConnectionFailureOverlayProps {
  failure: ConnectionFailure;
  onRetry?: () => void;
  presentation?: "overlay" | "section";
}

const ConnectionFailureOverlay = ({
  failure,
  onRetry,
  presentation = "overlay",
}: ConnectionFailureOverlayProps) => {
  const { t } = useTranslation();
  const title = t(`connections.failure.${failure.kind}.title`);
  const description = t(`connections.failure.${failure.kind}.description`);

  if (presentation === "section") {
    const isUnsupported = failure.kind === "unsupported";
    const Icon = isUnsupported ? Info : AlertTriangle;
    const content = (
      <>
        <Icon className="mt-0.5 h-3.5 w-3.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <p className="font-medium">{title}</p>
          <p className={isUnsupported ? "mt-0.5" : "mt-0.5 text-muted-foreground"}>
            {description}
          </p>
        </div>
        {onRetry && (
          <Button size="sm" variant="ghost" onClick={onRetry}>
            <RotateCw className="mr-1.5 h-3.5 w-3.5" />
            {t("connections.failure.retry")}
          </Button>
        )}
      </>
    );
    const className = `flex items-start gap-2 rounded-md border px-3 py-2 text-xs ${
      isUnsupported
        ? "border-border/40 bg-muted/40 text-muted-foreground"
        : "border-status-negative/25 bg-status-negative/10 text-status-negative"
    }`;
    return isUnsupported ? (
      <output className={className}>{content}</output>
    ) : (
      <div className={className} role="alert">
        {content}
      </div>
    );
  }

  return (
    <div
      className="absolute inset-0 flex flex-col items-center justify-center gap-2 bg-[hsl(220,15%,10%)]/90 p-4 text-center"
      role="alert"
    >
      <WifiOff className="h-8 w-8 text-status-negative" />
      <p className="max-w-xs text-sm font-semibold text-status-negative">{title}</p>
      <p className="max-w-sm text-xs text-muted-foreground">{description}</p>
      {onRetry && (
        <Button size="sm" variant="secondary" onClick={onRetry}>
          <RotateCw className="mr-2 h-4 w-4" />
          {t("connections.failure.retry")}
        </Button>
      )}
    </div>
  );
};

export default ConnectionFailureOverlay;
