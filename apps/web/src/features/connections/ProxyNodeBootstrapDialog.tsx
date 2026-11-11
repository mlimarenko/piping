import { useTranslation } from "react-i18next";
import { Copy } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogDescription } from "@/shared/ui-primitives/dialog";
import { notify } from "@/shared/lib/notifications";
import { writeBrowserClipboardText } from "@/shared/lib/clipboard";
import type { CreatedProxyNodeResponse, ProxyNodeBootstrap } from "@/features/connections/proxies-api";

interface Props {
  bootstrap: CreatedProxyNodeResponse | null;
  onClose: () => void;
}

function bootstrapEnvText(bootstrap: ProxyNodeBootstrap): string {
  return Object.entries(bootstrap.env)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

/** One-time reveal of a freshly created or rotated proxy-node secret. Shared by
 *  the node list (rotate) and the routed node editor (create), so the secret
 *  markup lives in exactly one place. */
export default function ProxyNodeBootstrapDialog({ bootstrap, onClose }: Readonly<Props>) {
  const { t } = useTranslation();
  const copyBootstrapValue = (value: string, successMessage: string) => {
    void writeBrowserClipboardText(value).then(
      () => notify("success", successMessage),
      (error) => notify("error", error instanceof Error ? error.message : t("unknownError")),
    );
  };
  return (
    <Dialog open={!!bootstrap} onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-[960px]">
        <DialogHeader>
          <DialogTitle className="text-xl">{t("proxyNodes.bootstrapTitle")}</DialogTitle>
          <DialogDescription>
            {t("proxyNodes.bootstrapDescription", { name: bootstrap?.node.name ?? "" })}
          </DialogDescription>
        </DialogHeader>
        {bootstrap && (
          <div className="flex min-w-0 flex-col gap-4 py-2">
            <div className="rounded-md border border-status-warning/30 bg-status-warning/10 px-3 py-2 text-xs text-status-warning">
              {t("proxyNodes.bootstrapWarning")}
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-1">
                <FieldLabel className="text-xs font-medium">{t("proxyNodes.nodeId")}</FieldLabel>
                <div className="rounded-md border border-border/40 bg-background px-2 py-1.5 font-mono text-[11px]">
                  {bootstrap.bootstrap.node_id}
                </div>
              </div>
              <div className="space-y-1">
                <div className="flex items-center justify-between gap-2">
                  <FieldLabel className="text-xs font-medium">{t("proxyNodes.nodeSecret")}</FieldLabel>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-9 gap-2 text-sm"
                    onClick={() =>
                      copyBootstrapValue(bootstrap.bootstrap.node_secret, t("proxyNodes.secretCopied"))
                    }
                  >
                    <Copy className="h-3 w-3" />
                    {t("copy")}
                  </Button>
                </div>
                <div className="max-h-20 overflow-auto rounded-md border border-border/40 bg-background px-2 py-1.5 font-mono text-[11px] break-all">
                  {bootstrap.bootstrap.node_secret}
                </div>
              </div>
            </div>

            <div className="space-y-1">
              <div className="flex items-center justify-between gap-2">
                <FieldLabel className="text-xs font-medium">{t("proxyNodes.envVars")}</FieldLabel>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-9 gap-2 text-sm"
                  onClick={() =>
                    copyBootstrapValue(bootstrapEnvText(bootstrap.bootstrap), t("proxyNodes.envCopied"))
                  }
                >
                  <Copy className="h-3 w-3" />
                  {t("copy")}
                </Button>
              </div>
              <pre className="max-h-36 overflow-auto rounded-md border border-border/40 bg-background p-2 font-mono text-[11px] text-foreground">
                {bootstrapEnvText(bootstrap.bootstrap)}
              </pre>
            </div>

            <div className="space-y-1">
              <div className="flex items-center justify-between gap-2">
                <FieldLabel className="text-xs font-medium">{t("proxyNodes.dockerRun")}</FieldLabel>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-9 gap-2 text-sm"
                  onClick={() =>
                    copyBootstrapValue(bootstrap.bootstrap.docker_run_command, t("proxyNodes.commandCopied"))
                  }
                >
                  <Copy className="h-3 w-3" />
                  {t("copy")}
                </Button>
              </div>
              <pre className="max-h-44 overflow-auto rounded-md border border-border/40 bg-background p-2 font-mono text-[11px] text-foreground whitespace-pre-wrap break-all">
                {bootstrap.bootstrap.docker_run_command}
              </pre>
            </div>
          </div>
        )}
        <DialogFooter>
          <Button size="sm" onClick={onClose}>
            {t("done")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
