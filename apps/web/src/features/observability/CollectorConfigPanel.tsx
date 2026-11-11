import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, Download, FileCode2 } from "lucide-react";

import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import { Button } from "@/shared/ui-primitives/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/shared/ui-primitives/select";
import { writeBrowserClipboardText } from "@/shared/lib/clipboard";
import { downloadTextFile } from "@/shared/lib/download";
import { notify } from "@/shared/lib/notifications";
import { buildCollectorArtifact, type CollectorMode, type CollectorTarget } from "./collectorConfig";

interface CollectorConfigPanelProps {
  collectorUrl: string;
  targets: CollectorTarget[];
  subject: string | null;
}

/**
 * The collector artifact for the targets above: a Prometheus/vmagent file the
 * operator copies or downloads. Generated from resolved discovery data, so it
 * never carries credentials — the token is referenced by file path.
 */
const CollectorConfigPanel = ({ collectorUrl, targets, subject }: CollectorConfigPanelProps) => {
  const { t } = useTranslation();
  const [mode, setMode] = useState<CollectorMode>("http_sd");
  const [copied, setCopied] = useState(false);
  const artifact = useMemo(
    () => buildCollectorArtifact(mode, collectorUrl, targets, subject),
    [collectorUrl, mode, subject, targets],
  );

  const copy = async () => {
    try {
      await writeBrowserClipboardText(artifact.yaml);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
      notify("success", t("observability.monitoring.config.copied"));
    } catch {
      notify("error", t("observability.monitoring.config.copyFailed"));
    }
  };

  return (
    <section className="flex flex-col gap-2 border-t border-border pt-4" aria-label={t("observability.monitoring.config.title")}>
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <h3 className="flex items-center gap-1.5 text-sm font-semibold text-foreground">
            <FileCode2 className="size-4" />
            {t("observability.monitoring.config.title")}
          </h3>
          <p className="text-xs text-muted-foreground">{t("observability.monitoring.config.hint")}</p>
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <Select value={mode} onValueChange={(value) => setMode(value as CollectorMode)}>
            <SelectTrigger className="h-8 w-[11.5rem]" aria-label={t("observability.monitoring.config.title")}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="http_sd">{t("observability.monitoring.config.modeHttpSd")}</SelectItem>
              <SelectItem value="direct">{t("observability.monitoring.config.modeDirect")}</SelectItem>
            </SelectContent>
          </Select>
          <Button variant="outline" size="sm" className="gap-1.5" onClick={() => void copy()}>
            {copied ? <Check className="size-3.5 text-status-positive" /> : <Copy className="size-3.5" />}
            {t("observability.monitoring.config.copy")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="gap-1.5"
            onClick={() => downloadTextFile(artifact.fileName, artifact.yaml, "application/yaml")}
          >
            <Download className="size-3.5" />
            {t("observability.monitoring.config.download", { name: artifact.fileName })}
          </Button>
        </div>
      </div>
      <SurfaceCard className="overflow-hidden">
        <pre className="max-h-[24rem] overflow-auto p-3 text-xs leading-5 text-muted-foreground">
          {artifact.yaml}
        </pre>
      </SurfaceCard>
    </section>
  );
};

export default CollectorConfigPanel;
