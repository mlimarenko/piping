import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Download, Loader2 } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/shared/ui-primitives/dialog";
import { API_BASE_URL, fetchWithSession } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";

/**
 * Backend caps the export window (default 2h, `PIPING_CCTV__MAX_EXPORT_WINDOW_SECS`).
 * This client-side bound mirrors the default and — unlike a plain early-error
 * check — is now load-bearing: the download below reads the whole clip into a
 * `Blob` (see `handleExport`) so its HTTP status can be checked and a real error
 * surfaced, instead of a native `<a download>` silently saving a failed request's
 * small error body as a bogus "clip.mp4". That trade bounds this tab's memory to
 * roughly one clip's worth of bytes, so the cap stays enforced client-side too (an
 * operator who genuinely needs a longer window still gets it — the server accepts
 * a raised `PIPING_CCTV__MAX_EXPORT_WINDOW_SECS` — just not through this control).
 */
const MAX_WINDOW_SECONDS = 2 * 60 * 60;
const DEFAULT_WINDOW_SECONDS = 60;
const MAX_WINDOW_HOURS = MAX_WINDOW_SECONDS / 3600;

/** `Date` → the `YYYY-MM-DDTHH:mm` a `datetime-local` input wants (local time). */
function toLocalInput(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/**
 * Recorded-clip export: pick a channel and a time window, download the MP4.
 *
 * The download is a same-origin GET the session cookie authenticates (no fetch/
 * blob plumbing) — an `<a download>` the click drives. The window is bounded to
 * the backend's cap; the default is the last minute of retained footage.
 */
/**
 * A window handed over from the archive player, so "watch this, now save it"
 * does not make the operator re-enter the range. `nonce` distinguishes two
 * requests for the same window, which must still reopen the dialog.
 */
export interface ExportSeed {
  channel: number;
  startMs: number;
  endMs: number;
  nonce: number;
}

function ClipExport({
  targetId,
  channelCount,
  defaultEnd,
  seed,
  onSeedConsumed,
}: Readonly<{
  targetId: string;
  channelCount: number | null;
  defaultEnd: string | null;
  /** Window to preload and open the dialog with, if any. */
  seed?: ExportSeed | null;
  /** Called once a seed has been applied, so the parent can clear it. */
  onSeedConsumed?: () => void;
}>) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [downloading, setDownloading] = useState(false);

  const [initialStart, initialEnd] = useMemo(() => {
    const end = defaultEnd ? new Date(defaultEnd) : new Date();
    const start = new Date(end.getTime() - DEFAULT_WINDOW_SECONDS * 1000);
    return [toLocalInput(start), toLocalInput(end)];
  }, [defaultEnd]);

  const [channel, setChannel] = useState(1);
  const [start, setStart] = useState(initialStart);
  const [end, setEnd] = useState(initialEnd);

  const channels = channelCount && channelCount > 1 ? channelCount : 1;

  // Adopt a window chosen elsewhere (the archive player) and open on it. Keyed
  // by nonce so exporting the same window twice still reopens the dialog.
  const appliedSeed = useRef<number | null>(null);
  useEffect(() => {
    if (!seed || appliedSeed.current === seed.nonce) return;
    appliedSeed.current = seed.nonce;
    setChannel(seed.channel);
    setStart(toLocalInput(new Date(seed.startMs)));
    setEnd(toLocalInput(new Date(seed.endMs)));
    setOpen(true);
    onSeedConsumed?.();
  }, [seed, onSeedConsumed]);

  const handleExport = async () => {
    const startDate = new Date(start);
    const endDate = new Date(end);
    if (Number.isNaN(startDate.getTime()) || Number.isNaN(endDate.getTime())) {
      notify("error", t("deviceDashboard.exportInvalidRange", "Enter a valid time range."), {
        source: "connections",
      });
      return;
    }
    const windowSeconds = (endDate.getTime() - startDate.getTime()) / 1000;
    if (windowSeconds <= 0) {
      notify("error", t("deviceDashboard.exportEndAfterStart", "End must be after start."), {
        source: "connections",
      });
      return;
    }
    if (windowSeconds > MAX_WINDOW_SECONDS) {
      notify(
        "error",
        t("deviceDashboard.exportTooLong", {
          defaultValue: "The window is longer than {{hours}} hours; pick a shorter range.",
          hours: MAX_WINDOW_HOURS,
        }),
        { source: "connections" },
      );
      return;
    }

    const params = new URLSearchParams({
      channel: String(channel),
      start: startDate.toISOString(),
      end: endDate.toISOString(),
    });
    const href = `${API_BASE_URL}/api/v1/cctv/${encodeURIComponent(targetId)}/export?${params.toString()}`;

    // Realtime playback: the download takes about as long as the clip is long.
    setDownloading(true);
    notify(
      "info",
      t("deviceDashboard.exportStarted", "Preparing the clip — this runs at recording speed."),
      { source: "connections" },
    );
    try {
      const response = await fetchWithSession(href, { credentials: "same-origin" });
      if (!response.ok) {
        // A failed export (busy/no-footage/window-too-long/device-down) still
        // answers with the canonical small JSON error envelope — surface its
        // message instead of leaving the operator to wonder why nothing downloaded.
        let message = response.statusText || `HTTP ${response.status}`;
        try {
          const body = (await response.json()) as { message?: unknown };
          if (typeof body.message === "string" && body.message) {
            message = body.message;
          }
        } catch {
          /* not JSON — keep the status-based message */
        }
        throw new Error(message);
      }
      const blob = await response.blob();
      const objectUrl = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = objectUrl;
      anchor.download = `clip-ch${channel}-${startDate.toISOString().replace(/[:.]/g, "")}.mp4`;
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      URL.revokeObjectURL(objectUrl);
      setOpen(false);
    } catch (error) {
      notify(
        "error",
        t("deviceDashboard.exportFailed", {
          defaultValue: "Export failed: {{reason}}",
          reason: error instanceof Error ? error.message : String(error),
        }),
        { source: "connections" },
      );
    } finally {
      setDownloading(false);
    }
  };

  const applyPreset = (minutes: number) => {
    const presetEnd = defaultEnd ? new Date(defaultEnd) : new Date();
    setEnd(toLocalInput(presetEnd));
    setStart(toLocalInput(new Date(presetEnd.getTime() - minutes * 60 * 1000)));
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button type="button" size="sm" className="gap-2">
          <Download className="h-4 w-4" />
          {t("deviceDashboard.exportVideoDialog", "Export video…")}
        </Button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("deviceDashboard.exportTitle", "Export a clip")}</DialogTitle>
          <DialogDescription>
            {t(
              "deviceDashboard.exportDescription",
              "Choose a camera and recorded interval. Long clips may take about as long as their duration.",
            )}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="flex flex-wrap gap-2">
            {[1, 5, 15, 30].map((minutes) => (
              <Button
                key={minutes}
                type="button"
                size="sm"
                variant="outline"
                disabled={downloading}
                onClick={() => applyPreset(minutes)}
              >
                {t("deviceDashboard.lastMinutes", "Last {{count}} min", { count: minutes })}
              </Button>
            ))}
          </div>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            <label className="flex flex-col gap-1 text-sm text-muted-foreground sm:col-span-2">
              {t("deviceDashboard.exportChannel", "Camera")}
              <select
                value={channel}
                disabled={downloading}
                onChange={(event) => setChannel(Number(event.target.value))}
                className="h-9 rounded-md border border-border bg-background px-3 text-sm text-foreground"
              >
                {Array.from({ length: channels }, (_unused, index) => index + 1).map((value) => (
                  <option key={value} value={value}>
                    {t("deviceDashboard.channelShort", "Ch")} {value}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1 text-sm text-muted-foreground">
              {t("deviceDashboard.exportFrom", "From")}
              <input
                type="datetime-local"
                value={start}
                disabled={downloading}
                onChange={(event) => setStart(event.target.value)}
                className="h-9 rounded-md border border-border bg-background px-3 text-sm text-foreground"
              />
            </label>
            <label className="flex flex-col gap-1 text-sm text-muted-foreground">
              {t("deviceDashboard.exportTo", "To")}
              <input
                type="datetime-local"
                value={end}
                disabled={downloading}
                onChange={(event) => setEnd(event.target.value)}
                className="h-9 rounded-md border border-border bg-background px-3 text-sm text-foreground"
              />
            </label>
          </div>
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" disabled={downloading} onClick={() => setOpen(false)}>
            {t("cancel", "Cancel")}
          </Button>
          <Button type="button" className="gap-2" disabled={downloading} onClick={handleExport}>
            {downloading ? <Loader2 className="h-4 w-4 animate-spin" /> : <Download className="h-4 w-4" />}
            {downloading
              ? t("deviceDashboard.exportPreparing", "Preparing clip…")
              : t("deviceDashboard.exportDownload", "Download clip")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export default ClipExport;
