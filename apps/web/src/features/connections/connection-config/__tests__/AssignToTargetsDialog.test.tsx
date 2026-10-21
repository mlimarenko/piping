import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

const connectionConfigMocks = vi.hoisted(() => ({
  bulkAssignRecord: vi.fn(),
  listAssignments: vi.fn(),
  listRecords: vi.fn(),
}));
const connectionsMocks = vi.hoisted(() => ({
  CONNECTION_TARGETS_QUERY_KEY: ["connections", "targets"] as const,
  listConnectionTargets: vi.fn(),
}));

vi.mock("../api", () => connectionConfigMocks);
vi.mock("../../api", () => connectionsMocks);

import AssignToTargetsDialog from "../AssignToTargetsDialog";

afterEach(() => {
  // Radix defers focus restoration until after unmount; finish that work in
  // this DOM realm rather than after Vitest has replaced it for another file.
  vi.useFakeTimers();
  cleanup();
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
});

function renderDialog({ open = true, recordId = "record-current" } = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const onOpenChange = vi.fn();
  const view = render(
    <QueryClientProvider client={client}>
      <AssignToTargetsDialog
        open={open}
        onOpenChange={onOpenChange}
        recordId={recordId}
      />
    </QueryClientProvider>,
  );
  const rerenderDialog = (nextOpen: boolean, nextRecordId: string) => {
    view.rerender(
      <QueryClientProvider client={client}>
        <AssignToTargetsDialog
          open={nextOpen}
          onOpenChange={onOpenChange}
          recordId={nextRecordId}
        />
      </QueryClientProvider>,
    );
  };
  return { onOpenChange, rerenderDialog };
}

beforeEach(() => {
  Object.values(connectionConfigMocks).forEach((value) => {
    if (typeof value === "function" && "mockReset" in value) value.mockReset();
  });
  connectionsMocks.listConnectionTargets.mockReset();

  connectionsMocks.listConnectionTargets.mockResolvedValue({
    items: [
      { id: "target-a", name: "Camera A", targetKind: "device", deviceId: "target-a" },
      { id: "target-b", name: "Camera B", targetKind: "device", deviceId: "target-b" },
    ],
    total: 2,
    limit: 2,
    offset: 0,
  });
  connectionConfigMocks.listRecords.mockResolvedValue([
    { id: "record-current", name: "Shared CCTV", protocol: "cctv" },
    { id: "record-other", name: "Archive CCTV", protocol: "cctv" },
    { id: "record-ssh", name: "SSH Admin", protocol: "ssh" },
  ]);
  connectionConfigMocks.listAssignments.mockResolvedValue([
    {
      id: "assignment-other",
      record_id: "record-other",
      scope_type: "device",
      scope_id: "target-a",
      weight: 10,
      enabled: true,
      created_at: "2026-01-01T00:00:00Z",
    },
    {
      id: "assignment-ssh",
      record_id: "record-ssh",
      scope_type: "device",
      scope_id: "target-a",
      weight: 50,
      enabled: true,
      created_at: "2026-01-01T00:00:00Z",
    },
  ]);
});

describe("AssignToTargetsDialog", () => {
  it("locks selection, search, weight and cancel until a deferred request settles", async () => {
    let rejectSave!: (error: Error) => void;
    connectionConfigMocks.bulkAssignRecord.mockImplementation(() => new Promise((_resolve, reject) => { rejectSave = reject; }));
    const {onOpenChange} = renderDialog();
    fireEvent.click(await screen.findByRole("button", {name: /Camera A/i}));
    fireEvent.change(screen.getByRole("spinbutton", {name: "Weight"}), {target: {value: "8"}});
    fireEvent.click(screen.getByRole("button", {name: /Assign to 1 target/i}));
    await waitFor(() => expect(connectionConfigMocks.bulkAssignRecord).toHaveBeenCalledWith("record-current", ["target-a"], 8));
    expect(screen.getByRole("button", {name: /Camera B/i})).toBeDisabled();
    expect(screen.getByRole("textbox")).toBeDisabled();
    expect(screen.getByRole("spinbutton", {name: "Weight"})).toBeDisabled();
    expect(screen.getByRole("button", {name: "Cancel"})).toBeDisabled();
    rejectSave(new Error("save failed"));
    await waitFor(() => expect(screen.getByRole("spinbutton", {name: "Weight"})).not.toBeDisabled());
    expect(screen.getByRole("spinbutton", {name: "Weight"})).toHaveValue(8);
    expect(screen.getByRole("button", {name: /Camera A/i})).toHaveAttribute("aria-pressed", "true");
    expect(onOpenChange).not.toHaveBeenCalled();
  });
  it("warns when a selected target has a competing direct assignment", async () => {
    renderDialog();

    fireEvent.click(await screen.findByRole("button", { name: /Camera A/i }));

    expect(screen.getByText(/Archive CCTV/)).toBeInTheDocument();
    expect(screen.queryByText(/SSH Admin/)).not.toBeInTheDocument();
    expect(screen.getByText(/competing assignment/i)).toBeInTheDocument();
  });

  it("submits selected targets and renders every per-target outcome", async () => {
    connectionConfigMocks.bulkAssignRecord.mockResolvedValue({
      assigned_count: 1,
      already_assigned_count: 0,
      failed_count: 1,
      items: [
        { target_id: "target-a", status: "assigned", assignment: null, error: null },
        { target_id: "target-b", status: "failed", assignment: null, error: "Target unavailable" },
      ],
    });
    renderDialog();

    fireEvent.click(await screen.findByRole("button", { name: /Camera A/i }));
    fireEvent.click(screen.getByRole("button", { name: /Camera B/i }));
    fireEvent.click(screen.getByRole("button", { name: /Assign to 2 targets/i }));

    await waitFor(() => {
      expect(connectionConfigMocks.bulkAssignRecord).toHaveBeenCalledWith(
        "record-current",
        ["target-a", "target-b"],
        0,
      );
    });
    expect(await screen.findByText("Target unavailable")).toBeInTheDocument();
    expect(screen.getByText("Assigned")).toBeInTheDocument();
    expect(screen.getByText("Failed")).toBeInTheDocument();
  });

  it("filters targets by their metadata and restores the list when search is cleared", async () => {
    connectionsMocks.listConnectionTargets.mockResolvedValue({
      items: [
        {
          id: "target-a",
          name: "Camera A",
          targetKind: "device",
          deviceId: "target-a",
          site: "North",
        },
        {
          id: "target-b",
          name: "Camera B",
          targetKind: "device",
          deviceId: "target-b",
          primaryIp: "192.0.2.20",
        },
      ],
      total: 2,
      limit: 2,
      offset: 0,
    });
    renderDialog();

    const search = await screen.findByRole("textbox", { name: "Search targets" });
    fireEvent.change(search, { target: { value: "192.0.2.20" } });

    expect(screen.queryByRole("button", { name: /Camera A/i })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Camera B/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Clear search" }));

    expect(screen.getByRole("button", { name: /Camera A/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Camera B/i })).toBeInTheDocument();
  });

  it("resets target, search, and weight state when the dialog switches records", async () => {
    const { rerenderDialog } = renderDialog();

    fireEvent.click(await screen.findByRole("button", { name: /Camera A/i }));
    fireEvent.change(screen.getByRole("textbox", { name: "Search targets" }), {
      target: { value: "Camera A" },
    });
    fireEvent.change(screen.getByRole("spinbutton", { name: "Weight" }), {
      target: { value: "7" },
    });
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    expect(screen.getByRole("spinbutton", { name: "Weight" })).toHaveValue(7);

    rerenderDialog(true, "record-other");

    expect(screen.getByRole("textbox", { name: "Search targets" })).toHaveValue("");
    expect(screen.getByRole("spinbutton", { name: "Weight" })).toHaveValue(0);
    expect(screen.getByText("0 selected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Camera B/i })).toBeInTheDocument();
  });

  it("shows a load failure and keeps submission unavailable", async () => {
    connectionsMocks.listConnectionTargets.mockRejectedValue(new Error("offline"));
    renderDialog();

    expect(
      await screen.findByText("Failed to load targets and assignments"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Assign to 0 targets" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /Camera A/i })).not.toBeInTheDocument();
  });

  it("removes a selected target on a repeated click", async () => {
    renderDialog();

    const target = await screen.findByRole("button", { name: /Camera A/i });
    fireEvent.click(target);
    expect(target).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(target);

    expect(target).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByText("0 selected")).toBeInTheDocument();
  });

  it("clears every selected target and disables submission", async () => {
    renderDialog();

    fireEvent.click(await screen.findByRole("button", { name: /Camera A/i }));
    fireEvent.click(screen.getByRole("button", { name: /Camera B/i }));
    expect(screen.getByText("2 selected")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Clear all" }));

    expect(screen.getByText("0 selected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Assign to 0 targets" })).toBeDisabled();
  });

  it("blocks dismissal during assignment and allows it after the result arrives", async () => {
    let resolveAssignment!: (value: {
      assigned_count: number;
      already_assigned_count: number;
      failed_count: number;
      items: never[];
    }) => void;
    connectionConfigMocks.bulkAssignRecord.mockReturnValue(
      new Promise((resolve) => {
        resolveAssignment = resolve;
      }),
    );
    const { onOpenChange } = renderDialog();

    fireEvent.click(await screen.findByRole("button", { name: /Camera A/i }));
    fireEvent.click(screen.getByRole("button", { name: "Assign to 1 target" }));
    await waitFor(() => {
      expect(connectionConfigMocks.bulkAssignRecord).toHaveBeenCalledTimes(1);
    });

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onOpenChange).not.toHaveBeenCalled();

    resolveAssignment({
      assigned_count: 0,
      already_assigned_count: 0,
      failed_count: 0,
      items: [],
    });
    fireEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
