import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { changeAppLanguage } from "@/shared/i18n";
import i18n from "i18next";
import AuditLog from "./AuditLog";
import type { AuditEntry } from "@/shared/api/types";
import { AllPermissions } from "@/test/permissions";

const apiMocks = vi.hoisted(() => ({
  listAuditLogs: vi.fn(),
  getAuditLog: vi.fn(),
  searchAuditTargets: vi.fn(),
}));

vi.mock("@/features/audit/api", () => ({
  listAuditLogs: apiMocks.listAuditLogs,
  getAuditLog: apiMocks.getAuditLog,
  searchAuditTargets: apiMocks.searchAuditTargets,
}));

vi.mock("@/shared/hooks/useTablePageSize", () => ({
  useTablePageSize: () => ({ value: 25, setValue: vi.fn() }),
}));

function renderAuditLog(ui: ReactElement = <AuditLog />) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const view = render(
    <QueryClientProvider client={client}>
      <AllPermissions>{ui}</AllPermissions>
    </QueryClientProvider>,
  );
  return { ...view, client };
}

function comboboxContaining(text: string): HTMLElement {
  const combobox = screen.getByText(text).closest<HTMLElement>("[role='combobox']");
  if (!combobox) throw new Error(`No combobox contains ${text}`);
  return combobox;
}

/** A hand-off promise the test resolves/rejects to control query timing. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** MCP tool-call entry: channel badge, decision result, `mcp.` info style. */
const mcpEntry: AuditEntry = {
  id: "evt-mcp-1",
  timestamp: "2026-07-15T10:00:00Z",
  actor: "Alice",
  actorKind: "mcp_agent",
  action: "mcp.tool.invoked",
  resourceType: "device",
  resourceId: "dev-1",
  context: { outcome: "succeeded", decision: "allowed", correlation: { request_id: "123e4567-e89b-12d3-a456-426614174000" }, display: {} },
  actorRef: { kind: "user", id: "u-1", label: "Alice" },
  targetRef: { kind: "device", id: "dev-1", label: "router01", secondary: "site-a" },
  details: { decision: "allowed", tool: "ssh.exec", protocol: "ssh", via: "CI token" },
};

/** Created entry: positive style, no decision/status → em-dash result. */
const createdEntry: AuditEntry = {
  id: "evt-pb-1",
  timestamp: "2026-07-15T09:00:00Z",
  actor: "Bob",
  actorKind: "user",
  action: "playbook.created",
  resourceType: "playbook",
  resourceId: "pb-1",
  context: { outcome: "unknown", correlation: {}, display: {} },
  actorRef: { kind: "user", id: "u-2", label: "Bob" },
  targetRef: { kind: "playbook", id: "pb-1", label: "Deploy web" },
  details: {},
};

/** Failed entry: negative style, result derived from `status`. */
const failedEntry: AuditEntry = {
  id: "evt-sess-1",
  timestamp: "2026-07-15T08:00:00Z",
  actor: "Carol",
  actorKind: "user",
  action: "connection.session.failed",
  resourceType: "connection_session",
  resourceId: "sess-1",
  context: { outcome: "failed", state: { domain: "session", value: "failed" }, correlation: {}, display: {} },
  actorRef: { kind: "user", id: "u-3", label: "Carol" },
  targetRef: { kind: "connection_session", id: "sess-1", label: "SSH to host9" },
  details: { status: "failed" },
};

/**
 * Wire shapes of the observer audit gate: the admission record is `unknown`
 * (authorization is known, a visible display is not), the stop is
 * `not_applicable` with a disconnected transport, and the denial is decided.
 */
const observeEntry = (id: string, action: string, context: AuditEntry["context"]): AuditEntry => ({
  id,
  timestamp: "2026-07-15T11:00:00Z",
  actor: "Dana",
  actorKind: "user",
  action,
  resourceType: "connection_session",
  resourceId: "sess-1",
  context,
  actorRef: { kind: "user", id: "u-4", label: "Dana" },
  targetRef: { kind: "connection_session", id: "sess-1", label: "SSH to host9" },
  details: {},
});
const observeStarted = observeEntry("evt-observe-1", "connection.session.observe.started", {
  outcome: "unknown", decision: "allowed", emitter: { kind: "api" }, correlation: { session_id: "sess-1" }, display: {},
});
const observeStopped = observeEntry("evt-observe-2", "connection.session.observe.stopped", {
  outcome: "not_applicable", state: { domain: "transport", value: "disconnected" }, emitter: { kind: "api" }, correlation: { session_id: "sess-1" }, display: {},
});
const observeDenied = observeEntry("evt-observe-3", "connection.session.observe.denied", {
  outcome: "denied", decision: "denied", emitter: { kind: "api" }, correlation: { session_id: "sess-1" }, display: {},
});
/** A refused second operator leg; the live session keeps running, so no state. */
const attachDenied = observeEntry("evt-attach-denied", "connection.session.websocket.attach_denied", {
  outcome: "denied", emitter: { kind: "system" }, correlation: { session_id: "sess-1" }, display: {},
});

const targetFilter = {
  id: "device-filter-1",
  kind: "device",
  label: "Edge camera",
  secondary: "198.51.100.10",
  target_type: "device",
};

function envelope(items: AuditEntry[], total = items.length) {
  return { items, total, limit: 25, offset: 0 };
}

beforeEach(() => {
  apiMocks.searchAuditTargets.mockResolvedValue([]);
  apiMocks.getAuditLog.mockImplementation(async (id: string) => {
    const response = await apiMocks.listAuditLogs.mock.results.at(-1)?.value;
    return response.items.find((item: AuditEntry) => item.id === id) ?? mcpEntry;
  });
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("AuditLog rendering", () => {
  it("PIP-205 shows Russian initiator and truthful target types with advanced actor filtering", async () => {
    await changeAppLanguage("ru");
    const entry = (id: string, resourceType: string): AuditEntry => ({ ...createdEntry, id, resourceType, resourceId: id, targetRef: undefined });
    apiMocks.listAuditLogs.mockResolvedValue(envelope([entry("target-1", "connection_target"), entry("target-2", "secret_variable"), { ...entry("target-3", "historical_kind"), action: "historical.action" }, entry("target-4", "connection.target")]));
    const view = renderAuditLog();
    try {
      expect(await screen.findByRole("columnheader", { name: /Инициатор/ })).toBeInTheDocument();
      expect(await screen.findByTitle("connection_target")).toHaveTextContent(i18n.t("accessControl.policies.resourceTypes.connection_target"));
      expect(screen.getByTitle("secret_variable")).toHaveTextContent(i18n.t("accessControl.policies.resourceTypes.secret_variable"));
      expect(screen.getByTitle("historical_kind")).toHaveTextContent("Неизвестный тип цели");
      expect(screen.getByTitle("connection.target")).toHaveTextContent("Неизвестный тип цели");
      expect(screen.getByTitle("historical.action")).toHaveTextContent("historical.action");
      fireEvent.click(screen.getAllByRole("button", { name: "Детали" })[1]!);
      const dialog = await screen.findByRole("dialog");
      expect(await within(dialog).findByText("Секретная переменная (secret_variable)")).toBeVisible();
      fireEvent.click(within(dialog).getByRole("button", { name: /закрыть|close/i }));
      expect(screen.queryByLabelText("ID инициатора")).toBeNull();
      expect(screen.getByRole("combobox", { name: "Канал" })).toBeInTheDocument();
      expect(screen.getByRole("combobox", { name: "Действие" })).toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Фильтры" }));
      expect(screen.getByLabelText("ID инициатора")).toBeInTheDocument();
      fireEvent.click(screen.getByRole("combobox", { name: "Тип цели" }));
      expect(await screen.findByRole("option", { name: i18n.t("accessControl.policies.resourceTypes.connection_target") })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Сбросить фильтры" })).toBeInTheDocument();
    } finally {
      view.unmount();
      await changeAppLanguage("en");
    }
  });
  it.each(["another-row", "empty", "error"])("returns to primary search after query refetch removes the opener (%s)", async state => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry, createdEntry]));
    const { client } = renderAuditLog();
    await screen.findByText("Alice");
    const opener = screen.getAllByRole("button", { name: "Details" })[0]!;
    fireEvent.click(opener);
    const dialog = await screen.findByRole("dialog");
    if (state === "error") apiMocks.listAuditLogs.mockRejectedValue(new Error("query failed"));
    else apiMocks.listAuditLogs.mockResolvedValue(envelope(state === "empty" ? [] : [createdEntry]));
    await act(async () => { await client.refetchQueries({ queryKey: ["audit"] }); });
    await waitFor(() => expect(opener).not.toBeInTheDocument());
    expect(dialog).toHaveTextContent(mcpEntry.id);
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.getByPlaceholderText("Search event, name or ID...")).toHaveFocus());
  });
  it("renders each entry's actor, action label, summary, and result", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry, createdEntry]));

    renderAuditLog();

    // Actors from both rows.
    expect(await screen.findByText("Alice")).toBeInTheDocument();
    expect(screen.getByText("Bob")).toBeInTheDocument();
    // Action labels resolve through auditLog.events.<event>.action.
    expect(screen.getByText("MCP tool call")).toBeInTheDocument();
    expect(screen.getByText("Playbook created")).toBeInTheDocument();
    expect(screen.queryByText("Alice used ssh.exec through CI token on router01: Allowed")).toBeNull();
    // The count badge in the header reflects the envelope total.
    expect(screen.getByText("2 items")).toBeInTheDocument();
  });

  it("distinguishes a recorded identity without a name from a missing initiator and preserves IDs in details", async () => {
    const id = "019ec6e2-1946-78c1-aae3-a8bc12345678";
    const unnamed: AuditEntry = { ...createdEntry, actor: id, actorRef: { kind: "user", id, label: id }, resourceId: id, targetRef: { kind: "device", id, label: id } };
    apiMocks.listAuditLogs.mockResolvedValue(envelope([unnamed, { ...createdEntry, id: "missing", actor: "", actorRef: undefined }]));
    renderAuditLog();
    expect(await screen.findByText("Name not recorded")).toBeVisible();
    expect(screen.getByText("Initiator not recorded")).toBeVisible();
    expect(screen.getByText("Target name not recorded")).toBeVisible();
    fireEvent.click(screen.getAllByRole("button", { name: "Details" })[0]!);
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getAllByText(id).length).toBeGreaterThanOrEqual(2));
  });

  it("names the system actor and secret-field actions without mistaking their inventory type for an unknown target", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([{
      ...createdEntry, actor: "", actorRef: undefined, actorKind: "system",
      action: "inventory.custom_fields.reveal", resourceType: "inventory:dcim.device",
      targetRef: { kind: "inventory:dcim.device", id: "device-1", label: "<img src=x onerror=alert(1)>" },
    }]));
    renderAuditLog();
    expect(await screen.findByText("System")).toBeVisible();
    expect(screen.getByTitle("inventory.custom_fields.reveal")).toHaveTextContent(/^Secret fields viewed$/);
    expect(screen.getByTitle("inventory:dcim.device")).toHaveTextContent("Device");
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeVisible();
    expect(document.querySelector("img[src='x']")).toBeNull();
  });

  it("applies quick filters on the server and restores the full history when toggled off", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));
    renderAuditLog();
    await screen.findByText("Alice");
    const errors = screen.getByRole("button", { name: "Errors and denials" });
    fireEvent.click(errors);
    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenLastCalledWith(expect.objectContaining({ outcomes: ["failed", "denied"], offset: 0 })));
    expect(errors).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(errors);
    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenLastCalledWith(expect.objectContaining({ outcomes: [] })));
    fireEvent.click(screen.getByRole("button", { name: "Session lifecycle" }));
    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenLastCalledWith(expect.objectContaining({ stateDomain: "session", stateValue: undefined })));
    fireEvent.click(screen.getByRole("button", { name: "Session lifecycle" }));
    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenLastCalledWith(expect.objectContaining({ stateDomain: undefined })));
  });

  it("labels the actor channel icon per actor_kind", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    // The channel glyph exposes an accessible "Channel: <kind>" label.
    expect(await screen.findByLabelText("Channel: MCP agent")).toBeInTheDocument();
  });

  it("keeps the raw event type inspectable alongside the action label", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry, createdEntry, failedEntry]));

    renderAuditLog();

    // Raw event names remain inspectable even when the action label is translated.
    const mcpChip = await screen.findByTitle("mcp.tool.invoked");
    const createdChip = screen.getByTitle("playbook.created");
    const failedChip = screen.getByTitle("connection.session.failed");

    expect(mcpChip).toBeInTheDocument();
    expect(createdChip).toBeInTheDocument();
    expect(failedChip).toBeInTheDocument();
  });

  it.each([
    ["en", { started: "Observation started", stopped: "Observation stopped", denied: "Observation denied", attachDenied: "WebSocket attach denied" }],
    ["ru", { started: "Наблюдение начато", stopped: "Наблюдение остановлено", denied: "Наблюдение отклонено", attachDenied: "Подключение WebSocket отклонено" }],
  ] as const)("localizes the observer events in %s while the wire identifiers stay inspectable", async (language, labels) => {
    await changeAppLanguage(language);
    apiMocks.listAuditLogs.mockResolvedValue(envelope([observeStarted, observeStopped, observeDenied, attachDenied]));
    const view = renderAuditLog();
    try {
      expect(await screen.findByTitle("connection.session.observe.started")).toHaveTextContent(labels.started);
      expect(screen.getByTitle("connection.session.observe.stopped")).toHaveTextContent(labels.stopped);
      expect(screen.getByTitle("connection.session.observe.denied")).toHaveTextContent(labels.denied);
      expect(screen.getByTitle("connection.session.websocket.attach_denied")).toHaveTextContent(labels.attachDenied);
    } finally {
      view.unmount();
      await changeAppLanguage("en");
    }
  });

  it("keeps observer admission unknown, the stop not applicable, and the denial decided", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([observeStarted, observeStopped, observeDenied]));
    renderAuditLog();

    // Admission proves authorization only, not that a display was visible.
    const started = (await screen.findByText("Observation started")).closest("tr")!;
    expect(within(started).getByText("Unconfirmed")).toBeInTheDocument();
    expect(within(started).queryByText("Allowed")).toBeNull();

    const stopped = screen.getByText("Observation stopped").closest("tr")!;
    expect(within(stopped).getByText("Not applicable")).toBeInTheDocument();
    expect(within(stopped).getByText("Lifecycle state: Transport / Disconnected")).toBeInTheDocument();

    const denied = screen.getByText("Observation denied").closest("tr")!;
    expect(within(denied).getByText("Denied")).toBeInTheDocument();
    fireEvent.click(within(denied).getByRole("button", { name: "Details" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("connection.session.observe.denied")).toBeInTheDocument();
    expect(within(dialog).getByTitle("denied")).toHaveTextContent("Denied");
  });

  it("shows a refused second operator leg as denied without ending the live session", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([attachDenied]));
    renderAuditLog();

    const row = (await screen.findByText("WebSocket attach denied")).closest("tr")!;
    expect(within(row).getByText("Denied")).toBeInTheDocument();
    // A refused attach leaves the session live: no lifecycle state may be shown.
    expect(within(row).queryByText(/Lifecycle state:/)).toBeNull();
    fireEvent.click(within(row).getByRole("button", { name: "Details" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("connection.session.websocket.attach_denied")).toBeInTheDocument();
    expect(within(dialog).queryByText(/Lifecycle state:/)).toBeNull();
  });

  it("uses outcome rather than metadata decision or state", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry, failedEntry]));

    renderAuditLog();

    // mcpEntry.details.decision = allowed → "Allowed".
    expect(await screen.findByText("Succeeded")).toBeInTheDocument();
    // failedEntry has no decision; details.status = failed → "Failed".
    expect(screen.getByText("Failed")).toBeInTheDocument();
  });

  it("does not infer success from a historical active state", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([{ ...createdEntry, context: { outcome: "unknown", state: { domain: "session", value: "active" }, correlation: {}, display: {} }, details: { status: "success", _omitted: true } }]));
    renderAuditLog();
    expect(await screen.findByText("Unconfirmed")).toBeInTheDocument();
    expect(screen.getByText("Lifecycle state: Session / Active")).toBeInTheDocument();
    expect(screen.queryByText("Succeeded")).toBeNull();
  });

  it.each([
    ["en", "Unconfirmed", "Lifecycle state: Session / Active", "Details", "Allowed", "Worker"],
    ["ru", "Результат не подтвержден", "Состояние: Сессия / Активна", "Детали", "Разрешено", "Обработчик"],
  ] as const)("localizes %s state separately from outcome and preserves wire codes", async (language, outcome, state, details, decision, emitter) => {
    await changeAppLanguage(language);
    const entry: AuditEntry = { ...createdEntry, context: { outcome: "unknown", state: { domain: "session", value: "active" }, decision: "allowed", emitter: { kind: "worker" }, correlation: {}, display: {} } };
    apiMocks.listAuditLogs.mockResolvedValue(envelope([entry]));
    const view = renderAuditLog();
    try {
      expect(await screen.findByText(outcome)).toBeInTheDocument();
      expect(screen.getByText(state)).toHaveAttribute("title", "session / active");
      fireEvent.click(screen.getByRole("button", { name: details }));
      expect(await screen.findByTitle("allowed")).toHaveTextContent(decision);
      expect(screen.getByTitle("worker")).toHaveTextContent(emitter);
      expect(screen.getAllByTitle("session / active")).toHaveLength(2);
    } finally {
      view.unmount();
      await changeAppLanguage("en");
    }
  });

  it("labels an unreported automation state without inferring a verdict", async () => {
    const entry: AuditEntry = {
      ...createdEntry,
      context: { outcome: "unknown", state: { domain: "automation", value: "unreported" }, correlation: {}, display: {} },
    };
    apiMocks.listAuditLogs.mockResolvedValue(envelope([entry]));
    renderAuditLog();
    expect(
      await screen.findByText("Lifecycle state: Automation / Unreported"),
    ).toHaveAttribute("title", "automation / unreported");
    expect(screen.getByText("Unconfirmed")).toBeInTheDocument();
    expect(screen.queryByText("Succeeded")).toBeNull();
    expect(screen.queryByText("Failed")).toBeNull();
  });

  it("labels an unrecognized lifecycle state without inferring an outcome", async () => {
    const context = { outcome: "unknown", state: { domain: "__proto__", value: "future" }, correlation: {}, display: {} } as unknown as AuditEntry["context"];
    apiMocks.listAuditLogs.mockResolvedValue(envelope([{ ...createdEntry, context }]));
    renderAuditLog();
    expect(await screen.findByText("Unconfirmed")).toBeInTheDocument();
    expect(screen.getByText("Lifecycle state: Unknown state")).toHaveAttribute("title", "__proto__ / future");
  });
});

describe("AuditLog loading / empty / error states", () => {
  it("shows the loading skeleton until data resolves (not the empty state)", async () => {
    const pending = deferred<ReturnType<typeof envelope>>();
    apiMocks.listAuditLogs.mockReturnValue(pending.promise);

    renderAuditLog();

    // While pending the empty state must NOT show — the skeleton branch is active.
    expect(screen.queryByText("No audit entries")).toBeNull();

    await act(async () => {
      pending.resolve(envelope([mcpEntry]));
      await pending.promise;
    });

    expect(await screen.findByText("Alice")).toBeInTheDocument();
  });

  it("renders the empty state when there are no entries", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([], 0));

    renderAuditLog();

    expect(await screen.findByText("No audit entries")).toBeInTheDocument();
    expect(screen.getByText("Audit events will appear here")).toBeInTheDocument();
    expect(screen.getByText("0 items")).toBeInTheDocument();
  });

  it("surfaces a retryable error and reloads on retry", async () => {
    apiMocks.listAuditLogs
      .mockRejectedValueOnce(new Error("boom"))
      .mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText("Failed to load data")).toBeInTheDocument();
    expect(within(alert).getByText("Could not load audit events.")).toBeInTheDocument();
    expect(within(alert).queryByText("boom")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    // The retry refetch clears the error branch and renders the row.
    expect(await screen.findByText("Alice")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(apiMocks.listAuditLogs).toHaveBeenCalledTimes(2);
  });
});

describe("AuditLog detail dialog", () => {
  it("explains omitted metadata without presenting the marker as a field", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([{ ...mcpEntry, details: { protocol: "ssh", _omitted: true } }]));
    renderAuditLog();
    fireEvent.click(await screen.findByRole("button", { name: "Details" }));
    const region = await screen.findByRole("region", { name: "Metadata" });
    expect(within(region).getByText("Some fields are omitted for privacy.")).toBeInTheDocument();
    expect(within(region).queryByText("_omitted")).toBeNull();
    expect(within(region).getByText("ssh")).toBeInTheDocument();
    fireEvent.click(within(region).getByText("Raw JSON"));
    await waitFor(() => expect(region.querySelector("pre")).toHaveTextContent('"_omitted": true'));
  });
  it("shows a safe detail error without exposing the backend message", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));
    apiMocks.getAuditLog.mockRejectedValueOnce(new Error("sensitive backend error"));
    renderAuditLog();
    fireEvent.click(await screen.findByRole("button", { name: "Details" }));
    expect(await screen.findByText("Could not load event details.")).toBeInTheDocument();
    expect(screen.queryByText("sensitive backend error")).toBeNull();
  });

  it("filters related events using the correlation ID", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry], 7));
    renderAuditLog();
    fireEvent.click(await screen.findByRole("button", { name: "Details" }));
    fireEvent.click(await screen.findByRole("button", { name: "Related events" }));
    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(expect.objectContaining({ requestId: "123e4567-e89b-12d3-a456-426614174000" })));
    expect(screen.getByText("7 items")).toBeInTheDocument();
  });
  it("opens the detail dialog with identity fields and readable metadata", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    await screen.findByText("Alice");
    fireEvent.click(screen.getByRole("button", { name: "Details" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Audit Detail")).toBeInTheDocument();
    // Log ID and raw event type rows.
    expect(await within(dialog).findByText("evt-mcp-1")).toBeInTheDocument();
    expect(within(dialog).getByText("mcp.tool.invoked")).toBeInTheDocument();
    expect(within(dialog).getByText("Metadata")).toBeInTheDocument();
    expect(dialog).toHaveTextContent("ssh.exec");
    expect(dialog.querySelector("pre")).toBeNull();
    fireEvent.click(within(dialog).getByText("Raw JSON"));
    await waitFor(() => expect(dialog.querySelector("pre")?.textContent).toBe(JSON.stringify(mcpEntry.details, null, 2)));
  });

  it("preserves nested fields, ordered array values and primitive type distinctions as text", async () => {
    const details = {
      nested: { enabled: false, count: 0, absent: null, textNull: "null", textFalse: "false", empty: "", "": "empty-key value" },
      values: [true, false, 0, "", null, {}, [], { literal: "<img src=x onerror=alert(1)>" }],
      emptyObject: {}, emptyArray: [],
      ["long-key".repeat(40)]: "long-value".repeat(100),
    };
    apiMocks.listAuditLogs.mockResolvedValue(envelope([{ ...mcpEntry, details }]));
    renderAuditLog();
    fireEvent.click(await screen.findByRole("button", { name: "Details" }));
    const region = await within(await screen.findByRole("dialog")).findByRole("region", { name: "Metadata" });
    function field(key: string) {
      const term = within(region).getByText(key, { selector: "dt" });
      return term.parentElement!.querySelector("dd")!;
    }
    expect(field("enabled")).toHaveTextContent("Booleanfalse");
    expect(field("count")).toHaveTextContent("Number0");
    expect(field("absent")).toHaveTextContent("Nullnull");
    expect(field("textNull")).toHaveTextContent("Stringnull");
    expect(field("textFalse")).toHaveTextContent("Stringfalse");
    expect(field("empty")).toHaveTextContent("StringEmpty string");
    expect(field('Empty key ("")')).toHaveTextContent("empty-key value");
    expect(field("emptyObject")).toHaveTextContent("Object (0 fields)");
    expect(field("emptyArray")).toHaveTextContent("Array (0 items)");
    const items = within(region).getAllByRole("listitem");
    expect(items).toHaveLength(8);
    expect(items.map(item => item.textContent)).toEqual([
      "Booleantrue", "Booleanfalse", "Number0", "StringEmpty string", "Nullnull",
      "Object (0 fields)", "Array (0 items)", expect.stringContaining("<img src=x onerror=alert(1)>"),
    ]);
    expect(region.querySelector("img, a")).toBeNull();
    expect(field("long-key".repeat(40))).toHaveTextContent("long-value".repeat(100));
    const nestedDisclosure = field("nested").querySelector("details")!;
    fireEvent.click(nestedDisclosure.querySelector("summary")!);
    expect(nestedDisclosure.open).toBe(false);
    fireEvent.click(nestedDisclosure.querySelector("summary")!);
    expect(nestedDisclosure.open).toBe(true);
  });

  it("bounds large structured metadata and keeps omitted values in complete raw JSON", async () => {
    const details = { values: Array.from({ length: 1000 }, (_, i) => `value-${i}`) };
    apiMocks.listAuditLogs.mockResolvedValue(envelope([{ ...mcpEntry, details }]));
    renderAuditLog();
    fireEvent.click(await screen.findByRole("button", { name: "Details" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("Metadata");
    expect(within(dialog).getAllByRole("listitem").length).toBeLessThanOrEqual(100);
    expect(dialog).toHaveTextContent("Structured view limited to 100 values");
    expect(within(dialog).queryByText("value-999")).toBeNull();
    fireEvent.click(within(dialog).getByText("Raw JSON"));
    await waitFor(() => expect(dialog.querySelector("pre")?.textContent).toBe(JSON.stringify(details, null, 2)));
  });

  it("uses the same display budget for deeply nested metadata", async () => {
    let details: Record<string, unknown> = { finalValue: "deep leaf" };
    for (let i = 0; i < 150; i += 1) details = { child: details };
    apiMocks.listAuditLogs.mockResolvedValue(envelope([{ ...mcpEntry, details }]));
    renderAuditLog();
    fireEvent.click(await screen.findByRole("button", { name: "Details" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("Metadata");
    expect(dialog.querySelectorAll("dt").length).toBeLessThanOrEqual(100);
    expect(dialog).toHaveTextContent("Structured view limited to 100 values");
    fireEvent.click(await within(dialog).findByText("Raw JSON"));
    await waitFor(() => expect(dialog.querySelector("pre")?.textContent).toBe(JSON.stringify(details, null, 2)));
  });

  it("resets disclosures and raw view for another record and restores the opening action focus", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry, createdEntry]));
    renderAuditLog();
    await screen.findByText("Alice");
    const triggers = screen.getAllByRole("button", { name: "Details" });
    expect(triggers).toHaveLength(2);
    fireEvent.click(triggers[0]!);
    let dialog = await screen.findByRole("dialog");
    fireEvent.click(await within(dialog).findByText("Raw JSON"));
    await waitFor(() => expect(dialog.querySelector("pre")).not.toBeNull());
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(triggers[0]).toHaveFocus());
    fireEvent.click(triggers[1]!);
    dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("evt-pb-1")).toBeInTheDocument();
    expect(dialog).toHaveTextContent("Object (0 fields)");
    expect(dialog.querySelector("pre")).toBeNull();
    expect(within(dialog).getByText("Raw JSON").closest("details")).not.toHaveAttribute("open");
  });
});

describe("AuditLog filtering and pagination", () => {
  it("combines channel and validated actor ID filters", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));
    renderAuditLog();
    fireEvent.click(screen.getByRole("combobox", { name: "Channel" }));
    fireEvent.click(await screen.findByRole("option", { name: /MCP agent/i }));
    fireEvent.click(screen.getByRole("combobox", { name: "Channel" }));
    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    const actor = screen.getByLabelText("Initiator ID");
    fireEvent.change(actor, { target: { value: "invalid" } });
    expect(actor).toHaveAttribute("aria-invalid", "true");
    fireEvent.change(actor, { target: { value: "123e4567-e89b-12d3-a456-426614174000" } });
    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(expect.objectContaining({ actorKinds: ["mcp_agent"], actorIds: ["123e4567-e89b-12d3-a456-426614174000"] })));
    fireEvent.click(screen.getByRole("button", { name: /initiator: 123e4567/i }));
    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(expect.objectContaining({ actorKinds: ["mcp_agent"], actorIds: [] })));
  });

  it("scopes target lookup by type and forwards target and result selections", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));
    apiMocks.searchAuditTargets.mockResolvedValue([targetFilter]);

    renderAuditLog();

    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    fireEvent.click(screen.getByRole("combobox", { name: "Target type" }));
    fireEvent.click(await screen.findByRole("option", { name: /^Device$/i }));
    fireEvent.click(screen.getByRole("combobox", { name: "Target type" }));

    fireEvent.click(comboboxContaining("Choose target..."));
    expect(await screen.findByRole("option", { name: /Edge camera/i })).toBeInTheDocument();
    expect(apiMocks.searchAuditTargets).toHaveBeenCalledWith(
      expect.objectContaining({ targetType: "device", limit: 30 }),
    );
    fireEvent.click(screen.getByRole("option", { name: /Edge camera/i }));

    fireEvent.click(screen.getByRole("combobox", { name: "Outcome" }));
    fireEvent.click(await screen.findByRole("option", { name: /^Failed$/i }));

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          targetTypes: ["device"],
          targetIds: ["device-filter-1"],
          outcomes: ["failed"],
        }),
      );
    });

    const targetChip = screen.getByRole("button", { name: /target: Edge camera/i });
    apiMocks.listAuditLogs.mockClear();
    fireEvent.click(targetChip);

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          targetTypes: ["device"],
          targetIds: [],
          outcomes: ["failed"],
        }),
      );
    });
    expect(screen.queryByRole("button", { name: /target: Edge camera/i })).toBeNull();
    const targetTypeChip = screen.getByRole("button", { name: /resource type: Device/i });
    const resultChip = screen.getByRole("button", { name: /outcome: Failed/i });

    apiMocks.listAuditLogs.mockClear();
    fireEvent.click(targetTypeChip);
    fireEvent.click(resultChip);

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ targetTypes: [], targetIds: [], outcomes: [] }),
      );
    });
    expect(screen.queryByRole("button", { name: /resource type: Device/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /outcome: Failed/i })).toBeNull();
  });

  it("forwards date filters and clears them", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));
    renderAuditLog();
    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    expect(screen.queryByLabelText("MCP tool")).toBeNull();
    fireEvent.change(screen.getByLabelText("From time"), { target: { value: "2026-08-10T08:00" } });
    fireEvent.change(screen.getByLabelText("To time"), { target: { value: "2026-08-10T09:00" } });
    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(expect.objectContaining({ from: expect.any(String), to: expect.any(String) })));
    fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));
    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(expect.objectContaining({ from: undefined, to: undefined })));
  });

  it("selects an event type from the toolbar and removes it through its chip", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));
    renderAuditLog();

    fireEvent.click(screen.getByRole("combobox", { name: "Action" }));
    fireEvent.click(await screen.findByRole("option", { name: "MCP tool call" }));
    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ eventTypes: ["mcp.tool.invoked"] }),
      );
    });

    apiMocks.listAuditLogs.mockClear();
    fireEvent.click(screen.getByRole("button", { name: /Action: MCP tool call/i }));
    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ eventTypes: [] }),
      );
    });
  });

  it("sends the selected event-type funnel value to the query", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenCalled());
    apiMocks.listAuditLogs.mockClear();

    // Open the Action-column funnel and check the "MCP tool call" value.
    const funnel = await screen.findByRole("button", { name: /filter:\s*action/i });
    fireEvent.click(funnel);
    const option = await screen.findByRole("button", { name: /^mcp tool call$/i });
    fireEvent.click(option);

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ eventTypes: ["mcp.tool.invoked"] }),
      );
    });
  });

  it("offers unreported as an automation lifecycle filter", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "Filters" }));

    fireEvent.change(screen.getByLabelText("State domain"), {
      target: { value: "automation" },
    });
    const stateValue = screen.getByLabelText("State value");
    expect(
      within(stateValue).getByRole("option", { name: "Unreported" }),
    ).toBeInTheDocument();

    fireEvent.change(stateValue, { target: { value: "unreported" } });
    await waitFor(() =>
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          stateDomain: "automation",
          stateValue: "unreported",
        }),
      ),
    );
  });

  it("applies a date preset, exercising the advanced panel and its filter chip", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenCalled());
    apiMocks.listAuditLogs.mockClear();

    // Expand the advanced-filters panel, then apply the "Last 24h" preset.
    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    fireEvent.click(await screen.findByRole("button", { name: "Last 24h" }));

    // A `created_from` bound is sent and a "From time: …" chip appears.
    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ from: expect.any(String) }),
      );
    });
    expect(screen.getByRole("button", { name: /^from time:/i })).toBeInTheDocument();
    // The advanced-filter count badge now reads 1 on the Filters toggle
    // (`^filters` avoids matching the "Clear filters" reset control).
    expect(screen.getByRole("button", { name: /^filters/i })).toHaveTextContent("1");
  });

  it("clears every active filter via the reset control", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    await waitFor(() => expect(apiMocks.listAuditLogs).toHaveBeenCalled());

    const reset = screen.getByRole("button", { name: "Clear filters" });
    // No active filters yet → the reset control is disabled.
    expect(reset).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    fireEvent.click(await screen.findByRole("button", { name: "Last hour" }));

    expect(screen.getByRole("button", { name: /^from time:/i })).toBeInTheDocument();
    expect(reset).toBeEnabled();

    fireEvent.click(reset);

    await waitFor(() => {
      expect(screen.queryByRole("button", { name: /^from time:/i })).toBeNull();
    });
    expect(reset).toBeDisabled();
  });

  it("shows a query chip for the search term and clears it on click", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    await screen.findByText("Alice");
    const searchBox = screen.getByPlaceholderText("Search event, name or ID...");
    fireEvent.change(searchBox, { target: { value: "router01" } });

    const chip = await screen.findByRole("button", { name: /query: router01/i });
    fireEvent.click(chip);

    await waitFor(() => {
      expect(screen.queryByRole("button", { name: /query: router01/i })).toBeNull();
    });
  });

  it("re-queries with the backend sort key when a sortable header is clicked", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    await screen.findByText("Alice");
    apiMocks.listAuditLogs.mockClear();

    // The Timestamp header maps to the backend `created_at` sort key; first
    // click sorts ascending.
    fireEvent.click(screen.getByRole("button", { name: "Timestamp" }));

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ sort: "created_at", order: "asc" }),
      );
    });
  });

  it("requests the next page offset when paginating", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry], 60));

    renderAuditLog();

    // Wait for the loaded row so the total (60) reaches the pagination footer and
    // the Next control becomes enabled (three pages at 25/page).
    await screen.findByText("Alice");
    apiMocks.listAuditLogs.mockClear();

    fireEvent.click(screen.getByRole("button", { name: "Next page" }));

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ limit: 25, offset: 25 }),
      );
    });
  });
});
