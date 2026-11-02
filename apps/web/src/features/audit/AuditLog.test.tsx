import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import AuditLog from "./AuditLog";
import type { AuditEntry } from "@/shared/api/types";
import { AllPermissions } from "@/test/permissions";

const apiMocks = vi.hoisted(() => ({
  listAuditLogs: vi.fn(),
  searchAuditTargets: vi.fn(),
}));

vi.mock("@/features/audit/api", () => ({
  listAuditLogs: apiMocks.listAuditLogs,
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
  summary: "mcp.tool.invoked",
  actorRef: { kind: "user", id: "u-1", label: "Alice" },
  actorVia: { kind: "api_token", id: "tok-1", label: "CI token" },
  resourceRef: { kind: "device", id: "dev-1", label: "router01", secondary: "site-a" },
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
  summary: "playbook.created",
  actorRef: { kind: "user", id: "u-2", label: "Bob" },
  resourceRef: { kind: "playbook", id: "pb-1", label: "Deploy web" },
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
  summary: "connection.session.failed",
  actorRef: { kind: "user", id: "u-3", label: "Carol" },
  resourceRef: { kind: "connection_session", id: "sess-1", label: "SSH to host9" },
  details: { status: "failed" },
};

const actorFilter = {
  id: "user-filter-1",
  kind: "user",
  label: "Dana Operator",
  secondary: "dana@example.test",
  target_type: "user",
};

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
});

afterEach(() => {
  vi.clearAllMocks();
});

describe("AuditLog rendering", () => {
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
    await waitFor(() => expect(screen.getByPlaceholderText("Search event, target, summary...")).toHaveFocus());
  });
  it("renders each entry's actor, action label, summary, and result", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry, createdEntry]));

    renderAuditLog();

    // Actors from both rows.
    expect(await screen.findByText("Alice")).toBeInTheDocument();
    expect(screen.getByText("Bob")).toBeInTheDocument();
    // The "via" line localizes the actor-via kind + label.
    expect(screen.getByText("via API token · CI token")).toBeInTheDocument();
    // Action labels resolve through auditLog.events.<event>.action.
    expect(screen.getByText("MCP tool call")).toBeInTheDocument();
    expect(screen.getByText("Playbook created")).toBeInTheDocument();
    // Interpolated summary for the MCP tool call.
    expect(
      screen.getByText("Alice used ssh.exec through CI token on router01: Allowed"),
    ).toBeInTheDocument();
    // The count badge in the header reflects the envelope total.
    expect(screen.getByText("2 items")).toBeInTheDocument();
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

  it("derives the result from decision, falling back to status", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry, failedEntry]));

    renderAuditLog();

    // mcpEntry.details.decision = allowed → "Allowed".
    expect(await screen.findByText("Allowed")).toBeInTheDocument();
    // failedEntry has no decision; details.status = failed → "Failed".
    expect(screen.getByText("Failed")).toBeInTheDocument();
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
    expect(within(alert).getByText("boom")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    // The retry refetch clears the error branch and renders the row.
    expect(await screen.findByText("Alice")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(apiMocks.listAuditLogs).toHaveBeenCalledTimes(2);
  });
});

describe("AuditLog detail dialog", () => {
  it("opens the detail dialog with identity fields and readable metadata", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    await screen.findByText("Alice");
    fireEvent.click(screen.getByRole("button", { name: "Details" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Audit Detail")).toBeInTheDocument();
    // Log ID and raw event type rows.
    expect(within(dialog).getByText("evt-mcp-1")).toBeInTheDocument();
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
    const region = within(await screen.findByRole("dialog")).getByRole("region", { name: "Metadata" });
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
    expect(dialog.querySelectorAll("dt").length).toBeLessThanOrEqual(100);
    expect(dialog).toHaveTextContent("Structured view limited to 100 values");
    fireEvent.click(within(dialog).getByText("Raw JSON"));
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
    fireEvent.click(within(dialog).getByText("Raw JSON"));
    await waitFor(() => expect(dialog.querySelector("pre")).not.toBeNull());
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(triggers[0]).toHaveFocus());
    fireEvent.click(triggers[1]!);
    dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("evt-pb-1");
    expect(dialog).toHaveTextContent("Object (0 fields)");
    expect(dialog.querySelector("pre")).toBeNull();
    expect(within(dialog).getByText("Raw JSON").closest("details")).not.toHaveAttribute("open");
  });
});

describe("AuditLog filtering and pagination", () => {
  it("combines channel and actor filters, then clears the actor without losing the channel", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));
    apiMocks.searchAuditTargets.mockResolvedValue([actorFilter]);

    renderAuditLog();

    fireEvent.click(screen.getByRole("combobox", { name: "Channel" }));
    fireEvent.click(await screen.findByRole("option", { name: /MCP agent/i }));
    fireEvent.click(screen.getByRole("combobox", { name: "Channel" }));

    fireEvent.click(comboboxContaining("Choose actor..."));
    fireEvent.click(await screen.findByRole("option", { name: /Dana Operator/i }));

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          actorKinds: ["mcp_agent"],
          actorIds: ["user-filter-1"],
        }),
      );
    });
    expect(screen.getByRole("button", { name: /channel: MCP agent/i })).toBeInTheDocument();
    const actorChip = screen.getByRole("button", { name: /actor: Dana Operator/i });

    apiMocks.listAuditLogs.mockClear();
    fireEvent.click(actorChip);

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ actorKinds: ["mcp_agent"], actorIds: [] }),
      );
    });
    expect(screen.queryByRole("button", { name: /actor: Dana Operator/i })).toBeNull();
    const channelChip = screen.getByRole("button", { name: /channel: MCP agent/i });

    apiMocks.listAuditLogs.mockClear();
    fireEvent.click(channelChip);

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ actorKinds: [], actorIds: [] }),
      );
    });
    expect(screen.queryByRole("button", { name: /channel: MCP agent/i })).toBeNull();
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

    fireEvent.click(screen.getByRole("combobox", { name: "Result" }));
    fireEvent.click(await screen.findByRole("option", { name: /^Failed$/i }));

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          targetTypes: ["device"],
          targetIds: ["device-filter-1"],
          results: ["failed"],
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
          results: ["failed"],
        }),
      );
    });
    expect(screen.queryByRole("button", { name: /target: Edge camera/i })).toBeNull();
    const targetTypeChip = screen.getByRole("button", { name: /resource type: Device/i });
    const resultChip = screen.getByRole("button", { name: /result: Failed/i });

    apiMocks.listAuditLogs.mockClear();
    fireEvent.click(targetTypeChip);
    fireEvent.click(resultChip);

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ targetTypes: [], targetIds: [], results: [] }),
      );
    });
    expect(screen.queryByRole("button", { name: /resource type: Device/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /result: Failed/i })).toBeNull();
  });

  it("forwards MCP and date filters together and clears the whole filter set", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));

    renderAuditLog();

    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    fireEvent.change(screen.getByLabelText("MCP tool"), { target: { value: "ssh.exec" } });

    fireEvent.click(screen.getByRole("combobox", { name: "MCP group" }));
    fireEvent.click(await screen.findByRole("option", { name: /^SSH$/i }));
    fireEvent.click(screen.getByRole("combobox", { name: "MCP group" }));

    fireEvent.click(screen.getByRole("combobox", { name: "Protocol" }));
    fireEvent.click(await screen.findByRole("option", { name: /^VNC$/i }));
    fireEvent.click(screen.getByRole("combobox", { name: "Protocol" }));

    fireEvent.click(screen.getByRole("combobox", { name: "Risk" }));
    fireEvent.click(await screen.findByRole("option", { name: /^Control$/i }));
    fireEvent.change(screen.getByLabelText("From time"), {
      target: { value: "2026-08-10T08:00" },
    });
    fireEvent.change(screen.getByLabelText("To time"), {
      target: { value: "2026-08-10T09:00" },
    });

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          mcpTool: "ssh.exec",
          mcpToolGroups: ["ssh"],
          mcpProtocols: ["vnc"],
          mcpRisks: ["control"],
          from: expect.any(String),
          to: expect.any(String),
        }),
      );
    });
    expect(screen.getByRole("button", { name: /MCP tool: ssh.exec/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /MCP group: SSH/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Protocol: VNC/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Risk: Control/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /From time: 2026-08-10T08:00/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /To time: 2026-08-10T09:00/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /MCP group: SSH/i }));
    fireEvent.click(screen.getByRole("button", { name: /Protocol: VNC/i }));
    fireEvent.click(screen.getByRole("button", { name: /Risk: Control/i }));

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          mcpTool: "ssh.exec",
          mcpToolGroups: [],
          mcpProtocols: [],
          mcpRisks: [],
          from: expect.any(String),
          to: expect.any(String),
        }),
      );
    });
    expect(screen.queryByRole("button", { name: /MCP group: SSH/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /Protocol: VNC/i })).toBeNull();
    expect(screen.queryByRole("button", { name: /Risk: Control/i })).toBeNull();

    apiMocks.listAuditLogs.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Clear filters" }));

    expect(screen.queryByRole("button", { name: /MCP tool: ssh.exec/i })).toBeNull();

    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          mcpTool: undefined,
          mcpToolGroups: [],
          mcpProtocols: [],
          mcpRisks: [],
          from: undefined,
          to: undefined,
        }),
      );
    });
  });

  it("clears the MCP tool and each date bound independently", async () => {
    apiMocks.listAuditLogs.mockResolvedValue(envelope([mcpEntry]));
    renderAuditLog();

    fireEvent.click(screen.getByRole("button", { name: "Filters" }));
    fireEvent.change(screen.getByLabelText("MCP tool"), { target: { value: "ssh.exec" } });
    fireEvent.change(screen.getByLabelText("From time"), {
      target: { value: "2026-08-10T08:00" },
    });
    fireEvent.change(screen.getByLabelText("To time"), {
      target: { value: "2026-08-10T09:00" },
    });
    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          mcpTool: "ssh.exec",
          from: expect.any(String),
          to: expect.any(String),
        }),
      );
    });

    apiMocks.listAuditLogs.mockClear();
    fireEvent.click(screen.getByRole("button", { name: /MCP tool: ssh.exec/i }));
    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({
          mcpTool: undefined,
          from: expect.any(String),
          to: expect.any(String),
        }),
      );
    });

    apiMocks.listAuditLogs.mockClear();
    fireEvent.click(screen.getByRole("button", { name: /From time: 2026-08-10T08:00/i }));
    fireEvent.click(screen.getByRole("button", { name: /To time: 2026-08-10T09:00/i }));
    await waitFor(() => {
      expect(apiMocks.listAuditLogs).toHaveBeenCalledWith(
        expect.objectContaining({ mcpTool: undefined, from: undefined, to: undefined }),
      );
    });
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
    const searchBox = screen.getByPlaceholderText("Search event, target, summary...");
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
