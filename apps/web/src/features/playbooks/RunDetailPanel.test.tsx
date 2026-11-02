import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import { AllPermissions } from "@/test/permissions";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";
import { RUN_GROUP_EVENTS_PAGE, RunDetailPanel } from "./RunDetailPanel";
import { RunEventsPanel } from "./components/run-detail/RunEventsPanel";
import type {
  PlaybookRun,
  PlaybookRunEvent,
  PlaybookRunGroupDetail,
  PlaybookRunStatus,
} from "./api";

const RUN_GROUP_DETAIL_LIMIT = 50_000;

const playbookApiMock = vi.hoisted(() => ({
  cancelPlaybookRun: vi.fn(),
  cancelPlaybookRunGroup: vi.fn(),
  getPlaybookRun: vi.fn(),
  getPlaybookRunGroup: vi.fn(),
  listPlaybookRunArtifacts: vi.fn(),
  listPlaybookRunEvents: vi.fn(),
  listPlaybookRunGroupEvents: vi.fn(),
}));

vi.mock("@/features/playbooks/api", () => ({
  cancelPlaybookRun: playbookApiMock.cancelPlaybookRun,
  cancelPlaybookRunGroup: playbookApiMock.cancelPlaybookRunGroup,
  getPlaybookRun: playbookApiMock.getPlaybookRun,
  getPlaybookRunGroup: playbookApiMock.getPlaybookRunGroup,
  listPlaybookRunArtifacts: playbookApiMock.listPlaybookRunArtifacts,
  listPlaybookRunEvents: playbookApiMock.listPlaybookRunEvents,
  listPlaybookRunGroupEvents: playbookApiMock.listPlaybookRunGroupEvents,
}));

function renderPanel(groupId = "group-1") {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  return render(
    <AllPermissions>
      <QueryClientProvider client={client}>
        <RunDetailPanel
          target={{ kind: "group", groupId }}
          playbookName="System update"
          visible
        />
      </QueryClientProvider>
    </AllPermissions>,
  );
}

function run(index: number, overrides: Partial<PlaybookRun> = {}): PlaybookRun {
  return {
    attempt: 1,
    credential_account_kind: "none",
    credential_merge_policy: "auto",
    credential_resolution: {},
    id: `run-${index}`,
    metadata: {},
    playbook_id: "playbook-1",
    playbook_version_id: "version-1",
    queued_at: "2026-07-08T00:00:00Z",
    run_group_id: "group-1",
    source_id: `source-${index}`,
    source_kind: "device",
    status: "running",
    target_address: `10.0.0.${index}`,
    target_address_bound: true,
    target_id: `target-${index}`,
    target_kind: "connection_target",
    target_name: `Target ${index}`,
    timeout_seconds: 1800,
    ...overrides,
  };
}

function groupDetail(
  status: PlaybookRunStatus,
  runs: PlaybookRun[],
  overrides: Partial<PlaybookRunGroupDetail["run_group"]> = {},
): PlaybookRunGroupDetail {
  const failedRuns = runs.filter((item) => item.status === "failed").length;
  const timeoutRuns = runs.filter((item) => item.status === "timed_out").length;
  const canceledRuns = runs.filter((item) => item.status === "canceled").length;
  const succeededRuns = runs.filter((item) => item.status === "succeeded").length;
  return {
    run_group: {
      canceled_runs: canceledRuns,
      created_at: "2026-07-08T00:00:00Z",
      failed_runs: failedRuns,
      id: "group-1",
      metadata: {},
      playbook_id: "playbook-1",
      playbook_version_id: "version-1",
      status,
      succeeded_runs: succeededRuns,
      timeout_runs: timeoutRuns,
      total_runs: runs.length,
      dispatch_id: null,
      dispatch_attempt: 0,
      trigger_kind: "manual",
      updated_at: "2026-07-08T00:00:00Z",
      ...overrides,
    },
    runs,
  };
}

describe("RunDetailPanel empty grouped output", () => {
  it.each(["survives", "another-artifact", "empty", "error", "inactive"])("restores artifact preview focus within its run (%s)", async state => {
    const artifact = { id: "artifact-1", run_id: "run-1", name: "result.json", artifact_type: "result", content_type: "application/json", content_size_bytes: 2, content_json: {}, metadata: {}, created_at: "2026-07-08T00:00:00Z" };
    playbookApiMock.listPlaybookRunArtifacts.mockResolvedValue({ items: [artifact] });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const content = (active: boolean) => <QueryClientProvider client={client}><RunEventsPanel run={run(1, { status: "succeeded" })} panelActive={active} pollPerRun={false} /></QueryClientProvider>;
    const view = render(content(true));
    const opener = await screen.findByRole("button", { name: /result.json/ });
    fireEvent.click(opener);
    const dialog = await screen.findByRole("dialog", { name: "result.json" });
    if (state === "inactive") view.rerender(content(false));
    else if (state !== "survives") {
      if (state === "error") playbookApiMock.listPlaybookRunArtifacts.mockRejectedValue(new Error("query failed"));
      else playbookApiMock.listPlaybookRunArtifacts.mockResolvedValue({ items: state === "empty" ? [] : [{ ...artifact, id: "artifact-2", name: "other.json" }] });
      await act(async () => { await client.refetchQueries({ queryKey: ["playbooks", "run-artifacts", "run-1"] }); });
    }
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    if (state === "survives" || state === "error") await waitFor(() => expect(opener).toHaveFocus());
    else if (state === "inactive") expect(screen.getByRole("region", { name: "Output" })).not.toHaveFocus();
    else await waitFor(() => expect(screen.getByRole("region", { name: "Output" })).toHaveFocus());
    client.clear();
  });
  beforeEach(() => {
    playbookApiMock.cancelPlaybookRun.mockReset();
    playbookApiMock.cancelPlaybookRunGroup.mockReset();
    playbookApiMock.getPlaybookRun.mockReset();
    playbookApiMock.getPlaybookRunGroup.mockReset();
    playbookApiMock.listPlaybookRunArtifacts.mockReset();
    playbookApiMock.listPlaybookRunEvents.mockReset();
    playbookApiMock.listPlaybookRunGroupEvents.mockReset();

    playbookApiMock.cancelPlaybookRun.mockResolvedValue({});
    playbookApiMock.cancelPlaybookRunGroup.mockResolvedValue({});
    playbookApiMock.listPlaybookRunArtifacts.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookRunEvents.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookRunGroupEvents.mockResolvedValue({
      items: [],
      total: 0,
      limit: 1000,
      offset: 0,
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it.each([
    ["PLAYBOOK_RUN", "group"], ["PLAYBOOK_RUN_GROUP", "individual"],
  ] as const)("does not treat %s READ as authority for %s requests or output", (resource, mode) => {
    const permissions: PermissionsValue = { roles: [], isAdmin: false, can: (action, type) => action === "READ" && type === resource, canAny: () => false, hasRole: () => false };
    render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><PermissionsContext.Provider value={permissions}>
      <RunDetailPanel target={mode === "group" ? { kind: "group", groupId: "group-1" } : { kind: "seed", runs: [run(1)] }} visible />
    </PermissionsContext.Provider></QueryClientProvider>);
    expect(screen.getByRole("alert")).toHaveTextContent("You don't have access");
    expect(playbookApiMock.getPlaybookRunGroup).not.toHaveBeenCalled();
    expect(playbookApiMock.getPlaybookRun).not.toHaveBeenCalled();
    expect(playbookApiMock.listPlaybookRunGroupEvents).not.toHaveBeenCalled();
    expect(playbookApiMock.listPlaybookRunEvents).not.toHaveBeenCalled();
    expect(playbookApiMock.listPlaybookRunArtifacts).not.toHaveBeenCalled();
  });

  it("does not show a false succeeded state before historical group details load", async () => {
    playbookApiMock.getPlaybookRunGroup.mockReturnValue(
      new Promise<PlaybookRunGroupDetail>(() => {}),
    );

    renderPanel();

    const panel = await screen.findByRole("region", { name: "Run detail" });
    expect(within(panel).getByText("Pending")).toBeInTheDocument();
    expect(within(panel).getAllByText("Loading...").length).toBeGreaterThan(0);
    expect(within(panel).queryByText("Succeeded")).not.toBeInTheDocument();
    expect(
      within(panel).queryByText("Run is still active. Output has not arrived yet."),
    ).not.toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "Stop" })).not.toBeInTheDocument();
  });

  it("shows aggregate active progress when all-target grouped output has not arrived", async () => {
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue(
      groupDetail("running", [
        run(1, { status: "succeeded" }),
        run(2, { status: "running" }),
      ]),
    );

    renderPanel();

    const panel = await screen.findByRole("region", { name: "Run detail" });
    expect(
      await within(panel).findByText("Run is still active. Output has not arrived yet."),
    ).toBeInTheDocument();
    expect(within(panel).getByText("1/2 targets finished")).toBeInTheDocument();
    expect(within(panel).queryByText("No events yet.")).not.toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.getPlaybookRunGroup).toHaveBeenCalledWith(
        "group-1",
        RUN_GROUP_DETAIL_LIMIT,
      );
    });
  });

  it("shows stop-requested aggregate context when every active target is stopping", async () => {
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue(
      groupDetail("running", [
        run(1, { status: "succeeded" }),
        run(2, {
          status: "running",
          cancel_requested_at: "2026-07-08T00:01:00Z",
        }),
      ]),
    );

    renderPanel();

    const panel = await screen.findByRole("region", { name: "Run detail" });
    expect(
      await within(panel).findByText("Stop requested. Waiting for the run to finish."),
    ).toBeInTheDocument();
    expect(within(panel).getByText("1/2 targets finished")).toBeInTheDocument();
  });

  it("keeps active mixed canceled groups in running progress state", async () => {
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue(
      groupDetail("running", [
        run(1, { status: "canceled" }),
        run(2, { status: "running" }),
      ]),
    );

    renderPanel();

    const panel = await screen.findByRole("region", { name: "Run detail" });
    expect(
      await within(panel).findByText("Run is still active. Output has not arrived yet."),
    ).toBeInTheDocument();
    expect(within(panel).getByText("1/2 targets finished")).toBeInTheDocument();
    expect(within(panel).getByText("1 canceled")).toBeInTheDocument();
    expect(
      within(panel).queryByText("Run was canceled before any output was received."),
    ).not.toBeInTheDocument();
    // The output stays in the non-terminal (running) state: no end marker yet.
    expect(within(panel).queryByTestId("run-output-end-marker")).not.toBeInTheDocument();
  });

  it("marks canceled all-target output and terminal footer as canceled instead of succeeded", async () => {
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue(
      groupDetail("canceled", [
        run(1, { status: "canceled" }),
        run(2, { status: "canceled" }),
      ]),
    );

    renderPanel();

    const panel = await screen.findByRole("region", { name: "Run detail" });
    expect(
      await within(panel).findByText("Run was canceled before any output was received."),
    ).toBeInTheDocument();
    expect(within(panel).getByText("2/2 targets finished")).toBeInTheDocument();
    expect(within(panel).getByText("2 canceled")).toBeInTheDocument();
    expect(within(panel).getByTestId("run-output-end-marker")).toHaveAttribute(
      "data-state",
      "canceled",
    );
    expect(within(panel).queryByText("Succeeded")).not.toBeInTheDocument();
  });
});

describe("RunGroupEventsPanel incremental accumulation", () => {
  // Single source of truth with the component: the loop-drain only continues
  // fetching while a page comes back full (items.length === RUN_GROUP_EVENTS_PAGE).
  const EVENTS_PAGE = RUN_GROUP_EVENTS_PAGE;

  function groupEvent(
    sequence: number,
    message: string,
    kind: "stdout" | "status" = "stdout",
  ): PlaybookRunEvent {
    return {
      created_at: "2026-07-08T00:00:00Z",
      event_kind: kind,
      event_sequence: sequence,
      event_type: kind,
      id: `event-${sequence}`,
      message,
      payload: null,
      run_group_id: "group-1",
      run_id: null,
    };
  }

  beforeEach(() => {
    playbookApiMock.getPlaybookRunGroup.mockReset();
    playbookApiMock.listPlaybookRunGroupEvents.mockReset();
    playbookApiMock.listPlaybookRunArtifacts.mockReset();
    playbookApiMock.listPlaybookRunEvents.mockReset();
    playbookApiMock.listPlaybookRunArtifacts.mockResolvedValue({ items: [] });
    playbookApiMock.listPlaybookRunEvents.mockResolvedValue({ items: [] });
    playbookApiMock.getPlaybookRunGroup.mockResolvedValue(
      groupDetail("running", [run(1)]),
    );
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("loop-drains the backlog via the cursor and shows output beyond the first page", async () => {
    // A FULL first page (EVENTS_PAGE items) is what makes the drain fetch a
    // second page. Only the head marker is console output (stdout, rendered);
    // the rest are non-console `status` events — they still count toward the
    // page's length (so the drain continues) but are filtered out of the render,
    // so the test never materialises 20k DOM nodes.
    const firstPage = Array.from({ length: EVENTS_PAGE }, (_, i) =>
      i === 0
        ? groupEvent(1, "FIRST-PAGE-HEAD-MARKER", "stdout")
        : groupEvent(i + 1, `filler-${i + 1}`, "status"),
    );
    playbookApiMock.listPlaybookRunGroupEvents
      .mockResolvedValueOnce({ items: firstPage, total: EVENTS_PAGE + 1, limit: EVENTS_PAGE, offset: 0 })
      .mockResolvedValueOnce({
        items: [groupEvent(EVENTS_PAGE + 1, "BEYOND-FIRST-PAGE-MARKER")],
        total: EVENTS_PAGE + 1,
        limit: EVENTS_PAGE,
        offset: 0,
      })
      .mockResolvedValue({ items: [], total: EVENTS_PAGE + 1, limit: EVENTS_PAGE, offset: 0 });

    renderPanel();

    // A line PAST the first page renders → the cursor+loop fetched the whole
    // backlog in one poll instead of truncating at the first page.
    expect(await screen.findByText(/BEYOND-FIRST-PAGE-MARKER/)).toBeInTheDocument();
    // The head of page one is still present → pages accumulate, never replace.
    expect(screen.getByText(/FIRST-PAGE-HEAD-MARKER/)).toBeInTheDocument();
    // The second fetch advanced the cursor to the last sequence of page one.
    expect(playbookApiMock.listPlaybookRunGroupEvents).toHaveBeenNthCalledWith(
      2,
      "group-1",
      expect.objectContaining({ afterSequence: EVENTS_PAGE }),
    );
  });
});
