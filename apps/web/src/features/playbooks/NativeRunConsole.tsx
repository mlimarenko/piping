import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { ApiError } from "@/shared/api/client";
import { getRunGroupConsole } from "./api";
import { findConsoleText } from "./NativeRunConsoleModel";
import { ConsoleConflict, DurableConsoleModel } from "./DurableConsoleModel";
import { SYNTHETIC_RUN_CHUNKS } from "@/demo/native-run-fixture";

const EXPORT_LIMIT = 64 * 1024 * 1024;
const DEMO_DISPATCH = "00000000-0000-4000-8000-000000000199";

async function page(groupId: string, cursor: number, dispatchId: string | null, signal: AbortSignal, demo: boolean): Promise<unknown> {
  if (!demo) return getRunGroupConsole(groupId, { fromSequence: cursor, ...(dispatchId ? { dispatchId } : {}), signal });
  const chunks = SYNTHETIC_RUN_CHUNKS.filter((chunk) => chunk.sequence >= cursor).map((chunk) => ({ ...chunk, stream: "stdout", bytes: Array.from(chunk.bytes) }));
  return { dispatch_id: DEMO_DISPATCH, dispatch_attempt: 1, chunks, next_sequence: cursor + chunks.length, complete: true };
}

export function NativeRunConsole({ groupId, label, demo = false }: { groupId: string; label: string; demo?: boolean }) {
  return <ConsoleSession key={`${groupId}:${demo}`} groupId={groupId} label={label} demo={demo} />;
}

function ConsoleSession({ groupId, label, demo }: { groupId: string; label: string; demo: boolean }) {
  const { t } = useTranslation();
  const searchId = useId();
  const element = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const modelRef = useRef<DurableConsoleModel | null>(null);
  const followRef = useRef(true);
  const pausedRow = useRef(0);
  const searchRow = useRef(0);
  const [following, setFollowing] = useState(true);
  const [query, setQuery] = useState("");
  const [notice, setNotice] = useState("");
  const [error, setError] = useState<"failed" | "conflict" | null>(null);
  const [retry, setRetry] = useState(0);
  const [complete, setComplete] = useState(false);
  const [hasDispatch, setHasDispatch] = useState(false);
  const [exporting, setExporting] = useState(false);
  const exportAbort = useRef<AbortController | null>(null);

  useEffect(() => {
    if (!element.current) return;
    const terminal = new Terminal({ disableStdin: true, scrollback: 5000, convertEol: false, theme: { background: "#0b0d10" } });
    const fit = new FitAddon();
    terminal.loadAddon(fit); terminal.open(element.current); fit.fit();
    terminalRef.current = terminal;
    modelRef.current = new DurableConsoleModel((text) => terminal.write(text, () => {
      if (followRef.current) terminal.scrollToBottom(); else terminal.scrollToLine(pausedRow.current);
    }));
    const observer = new ResizeObserver(() => fit.fit()); observer.observe(element.current);
    return () => { exportAbort.current?.abort(); observer.disconnect(); modelRef.current = null; terminalRef.current = null; terminal.dispose(); };
  }, [groupId, demo]);

  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const model = modelRef.current;
    if (!model) return () => controller.abort();
    const poll = async () => {
      try {
        const data = await page(groupId, model.cursor, model.dispatchId, controller.signal, demo);
        if (controller.signal.aborted) return;
        const received = model.accept(data);
        setError(null); setComplete(model.complete); setHasDispatch(model.dispatchId !== null);
        if (!model.complete) timer = setTimeout(() => { void poll(); }, received.chunks.length ? 0 : 1500);
      } catch (err) {
        if (!controller.signal.aborted) {
          const conflict = err instanceof ConsoleConflict || (err instanceof ApiError && err.status === 409);
          if (conflict) { exportAbort.current?.abort(); exportAbort.current = null; setExporting(false); }
          setError(conflict ? "conflict" : "failed");
        }
      }
    };
    void poll();
    return () => { controller.abort(); if (timer) clearTimeout(timer); };
  }, [groupId, demo, retry]);

  const exportPrefix = async (download: boolean) => {
    const source = modelRef.current;
    if (!source || source.dispatchId === null || error === "conflict" || exporting || exportAbort.current) return;
    const target = source.cursor;
    const dispatch = source.dispatchId;
    const sealed = source.complete;
    const controller = new AbortController(); exportAbort.current = controller; setExporting(true);
    try {
      const reader = new DurableConsoleModel(() => {});
      const parts: Uint8Array[] = [];
      let size = 0;
      while (reader.cursor < target) {
        const result = reader.accept(await page(groupId, reader.cursor, dispatch, controller.signal, demo));
        if (controller.signal.aborted) return;
        if (reader.dispatchId !== dispatch || reader.dispatchAttempt !== source.dispatchAttempt || result.chunks.length === 0) throw new Error("Console prefix changed");
        for (const chunk of result.chunks) { if (chunk.sequence >= target) break; size += chunk.bytes.length; if (size > EXPORT_LIMIT) throw new Error("Export limit exceeded"); parts.push(chunk.bytes); }
      }
      const bytes = new Uint8Array(size); let offset = 0;
      for (const part of parts) { bytes.set(part, offset); offset += part.length; }
      if (download) {
        const url = URL.createObjectURL(new Blob([bytes.buffer], { type: "application/octet-stream" }));
        const link = document.createElement("a"); link.href = url; link.download = sealed ? "run-output.bin" : `run-output-prefix-${target}.bin`; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
      } else await navigator.clipboard.writeText(new TextDecoder().decode(bytes));
      if (!controller.signal.aborted) setNotice(sealed ? t("playbooks.nativeConsole.exported", { defaultValue: "Complete output exported." }) : t("playbooks.nativeConsole.prefixExported", { defaultValue: "Known output prefix exported; the run may produce more." }));
    } catch { if (!controller.signal.aborted) setNotice(t("playbooks.nativeConsole.exportFailed", { defaultValue: "Export failed or exceeded the 64 MiB safety limit." })); }
      finally { if (exportAbort.current === controller) { exportAbort.current = null; if (!controller.signal.aborted) setExporting(false); } }
  };

  return <section className="flex min-h-0 flex-col gap-2">
    <div className="flex flex-wrap items-center gap-2">
      <Button variant="outline" type="button" aria-pressed={following} onClick={() => { const next = !followRef.current; followRef.current = next; setFollowing(next); if (next) terminalRef.current?.scrollToBottom(); else pausedRow.current = terminalRef.current?.buffer.active.viewportY ?? 0; }}>{following ? t("playbooks.nativeConsole.pause", { defaultValue: "Pause follow" }) : t("playbooks.nativeConsole.follow", { defaultValue: "Follow" })}</Button>
      <Button variant="outline" type="button" disabled={!hasDispatch || error === "conflict" || exporting} onClick={() => void exportPrefix(false)}>{t("playbooks.nativeConsole.copy", { defaultValue: "Copy" })}</Button>
      <Button variant="outline" type="button" disabled={!hasDispatch || error === "conflict" || exporting} onClick={() => void exportPrefix(true)}>{t("playbooks.nativeConsole.download", { defaultValue: "Download raw" })}</Button>
      <label className="sr-only" htmlFor={searchId}>{t("playbooks.nativeConsole.search", { defaultValue: "Search output" })}</label>
      <Input id={searchId} className="min-w-0 max-w-48" value={query} onChange={(event) => { setQuery(event.target.value); searchRow.current = 0; }} />
      <Button variant="outline" type="button" disabled={!query} onClick={() => { const term = terminalRef.current; if (!term) return; const row = findConsoleText(term, query, searchRow.current); searchRow.current = row === null ? 0 : row + 1; if (row !== null) { followRef.current = false; setFollowing(false); pausedRow.current = term.buffer.active.viewportY; } setNotice(row === null ? t("playbooks.nativeConsole.notFound", { defaultValue: "No match" }) : t("playbooks.nativeConsole.match", { defaultValue: "Match selected" })); }}>{t("playbooks.nativeConsole.findNext", { defaultValue: "Find next" })}</Button>
    </div>
    <p className="text-xs text-muted-foreground">{t("playbooks.nativeConsole.scrollbackLimit", { defaultValue: "Terminal scrollback is limited to 5,000 lines." })}</p>
    {!complete ? <p role="status">{t("playbooks.nativeConsole.prefix", { defaultValue: "Exports contain the known output prefix until the console is sealed." })}</p> : null}
    {error ? <div role="alert">{error === "conflict" ? t("playbooks.nativeConsole.conflict", { defaultValue: "Console dispatch changed. Output was stopped to avoid mixing runs." }) : t("playbooks.nativeConsole.loadFailed", { defaultValue: "Console output could not be loaded." })}{error === "failed" ? <Button variant="outline" type="button" onClick={() => setRetry((value) => value + 1)}>{t("playbooks.nativeConsole.retry", { defaultValue: "Retry" })}</Button> : null}</div> : null}
    {notice ? <p role="status">{notice}</p> : null}
    <div role="region" aria-label={label} className="h-64 min-w-0 overflow-hidden rounded-md" ref={element} />
  </section>;
}
