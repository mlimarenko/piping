import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { MemoryRouter, Route, Routes } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";
import DeviceUserSection from "@/features/device-dashboard/DeviceUserSection";
import DeviceDashboardPage from "@/features/device-dashboard/DeviceDashboardPage";
import type {
  DeviceDisk,
  DeviceInfo,
  DeviceRecording,
  DeviceUsers,
  DeviceUsersOutcome,
  DeviceUsersPreview,
} from "@/features/device-dashboard/api";

const apiMock = vi.hoisted(() => ({
  getDeviceCapabilities: vi.fn(),
  getDeviceInfo: vi.fn(),
  getDeviceUsers: vi.fn(),
  getDeviceDisk: vi.fn(),
  getDeviceRecording: vi.fn(),
  previewDeviceUsers: vi.fn(),
  createDeviceUsers: vi.fn(),
  updateDeviceUsers: vi.fn(),
  deleteDeviceUsers: vi.fn(),
  getDeviceAuthorities: vi.fn(),
}));
vi.mock("@/features/device-dashboard/api", () => apiMock);

// DeviceDashboardPage resolves the target's identity for its header before it
// renders the console. That call lives in the connections api, not the device
// api mocked above, so without this the page never leaves its skeleton and
// every assertion below times out instead of failing with a useful message.
const connectionsApiMock = vi.hoisted(() => ({
  CONNECTION_TARGETS_QUERY_KEY: ["connections", "targets"] as const,
  getConnectionTarget: vi.fn(),
}));
vi.mock("@/features/connections/api", () => connectionsApiMock);

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({ notify: notifyMock }));

const TARGET_ID = "target-1";

const USERS: DeviceUsers["users"] = [
  { name: "admin", group: "Administrators", authorities: ["all"] },
  { name: "operator", group: "Operators", authorities: ["live"] },
];

const okPreview = (over: Partial<DeviceUsersPreview["per_item"][number]>): DeviceUsersPreview => ({
  writes_enabled: true,
  per_item: [{ name: "cam-op", action: "create", reason: "New account", ...over }],
});

const okOutcome = (over: Partial<DeviceUsersOutcome>): DeviceUsersOutcome => ({
  succeeded: [],
  failed: [],
  all_succeeded: true,
  ...over,
});

function renderSection(
  canCreate: boolean,
  canDelete: boolean,
  users: DeviceUsers["users"] = [],
  hasAuthorityCatalog = false,
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <DeviceUserSection
        targetId={TARGET_ID}
        users={users}
        canCreate={canCreate}
        canDelete={canDelete}
        canUpdate={canCreate}
        hasAuthorityCatalog={hasAuthorityCatalog}
      />
    </QueryClientProvider>,
  );
}

// The gating test drives the real page so the capabilities-permissions ->
// `canManage` wiring is exercised, not just the section prop.
const INFO: DeviceInfo = { model: "NVR-16", firmware: "V5", serial: "S1", channels: 8, device_time: null };
const DISK: DeviceDisk = { disks: [] };
const RECORDING: DeviceRecording = { active: false, oldest: null, newest: null, spans: [] };

const caps = (canManage: boolean) => ({
  info: true,
  users: true,
  disk: true,
  recording: true,
  snapshot: false,
  streaming: false,
  channels: false,
  ptz: false,
  export: false,
  authority_catalog: false,
  recording_search: false,
  user_update: true,
  // The server's per-target RBAC verdicts drive the page's `canManage`.
  permissions: {
    view: false,
    recordings: false,
    export: false,
    users: canManage,
    settings: false,
    ptz: false,
  },
});

function permissions(): PermissionsValue {
  return {
    roles: ["viewer"],
    isAdmin: false,
    can: () => true,
    canAny: () => true,
    hasRole: () => false,
  };
}

function renderPage(canManage: boolean) {
  connectionsApiMock.getConnectionTarget.mockResolvedValue({
    id: TARGET_ID,
    name: "North gate recorder",
    targetKind: "device",
    primaryIp: "198.51.100.24",
    family: "dvrip_sofia",
  });
  apiMock.getDeviceCapabilities.mockResolvedValue(caps(canManage));
  apiMock.getDeviceInfo.mockResolvedValue(INFO);
  apiMock.getDeviceUsers.mockResolvedValue({ users: USERS });
  apiMock.getDeviceDisk.mockResolvedValue(DISK);
  apiMock.getDeviceRecording.mockResolvedValue(RECORDING);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <PermissionsContext.Provider value={permissions()}>
        <MemoryRouter initialEntries={[`/connections/devices/${TARGET_ID}`]}>
          <Routes>
            <Route path="/connections/devices/:targetId" element={<DeviceDashboardPage />} />
          </Routes>
        </MemoryRouter>
      </PermissionsContext.Provider>
    </QueryClientProvider>,
  );
}

describe("DeviceUserSection", () => {
  beforeEach(() => {
    Object.values(apiMock).forEach((m) => m.mockReset());
    notifyMock.mockReset();
  });

  it("edits an existing account only when update is available", async () => {
    apiMock.updateDeviceUsers.mockResolvedValue(okOutcome({ succeeded: ["operator"] }));

    renderSection(true, true, USERS);

    fireEvent.pointerDown(screen.getByRole("button", { name: /actions: operator/i }), { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /edit operator/i }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText(/password/i), {
      target: { value: "new-password" },
    });
    fireEvent.change(within(dialog).getByLabelText(/group/i), {
      target: { value: "Supervisors" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /save/i }));

    await waitFor(() =>
      expect(apiMock.updateDeviceUsers).toHaveBeenCalledWith(TARGET_ID, [
        {
          name: "operator",
          password: "new-password",
          group: "Supervisors",
        },
      ]),
    );
    expect(await within(dialog).findByText(/succeeded/i)).toBeInTheDocument();
  });

  it("does not duplicate an account update or open another write while it is pending", async () => {
    let finish!: (outcome: DeviceUsersOutcome) => void;
    apiMock.updateDeviceUsers.mockReturnValue(new Promise<DeviceUsersOutcome>(resolve => { finish = resolve; }));
    renderSection(true, true, USERS);
    fireEvent.pointerDown(screen.getByRole("button", { name: /actions: operator/i }), { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /edit operator/i }));
    const dialog = screen.getByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText(/group/i), { target: { value: "Supervisors" } });
    const form = within(dialog).getByLabelText(/group/i).closest("form");
    expect(form).not.toBeNull();
    fireEvent.submit(form!);
    fireEvent.submit(form!);
    await waitFor(() => expect(apiMock.updateDeviceUsers).toHaveBeenCalledTimes(1));
    fireEvent.click(within(dialog).getByRole("button", { name: /cancel/i }));
    expect(screen.getByRole("button", { name: /add user/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /actions: operator/i })).toBeDisabled();
    await act(async () => {
      finish(okOutcome({ succeeded: ["operator"] }));
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.getByRole("button", { name: /add user/i })).toBeEnabled());
    expect(apiMock.createDeviceUsers).not.toHaveBeenCalled();
    expect(apiMock.deleteDeviceUsers).not.toHaveBeenCalled();
  });

  it("previews then applies an add: create payload + success outcome", async () => {
    apiMock.previewDeviceUsers.mockResolvedValue(okPreview({}));
    apiMock.createDeviceUsers.mockResolvedValue(okOutcome({ succeeded: ["cam-op"] }));

    renderSection(true, true);

    fireEvent.click(screen.getByRole("button", { name: /add user/i }));
    const addDialog = screen.getByRole("dialog");
    fireEvent.change(within(addDialog).getByLabelText(/name/i), { target: { value: "cam-op" } });
    fireEvent.change(within(addDialog).getByLabelText(/password/i), { target: { value: "s3cret" } });
    fireEvent.change(within(addDialog).getByLabelText(/group/i), { target: { value: "Operators" } });
    fireEvent.click(within(addDialog).getByRole("button", { name: /preview/i }));

    const previewDialog = await screen.findByRole("dialog", { name: /add device user/i });
    // Dry-run verdict is shown before any write reaches the device.
    expect(await within(previewDialog).findByText("cam-op")).toBeInTheDocument();
    expect(within(previewDialog).getByText("Create")).toBeInTheDocument();
    expect(apiMock.createDeviceUsers).not.toHaveBeenCalled();

    fireEvent.click(within(previewDialog).getByRole("button", { name: /confirm/i }));

    await waitFor(() =>
      expect(apiMock.createDeviceUsers).toHaveBeenCalledWith(TARGET_ID, [
        { name: "cam-op", password: "s3cret", group: "Operators" },
      ]),
    );
    expect(await within(previewDialog).findByText(/succeeded/i)).toBeInTheDocument();
  });

  it("shows a skip verdict for an existing account", async () => {
    apiMock.previewDeviceUsers.mockResolvedValue(
      okPreview({ name: "admin", action: "skip", reason: "Account already exists" }),
    );

    renderSection(true, true);

    fireEvent.click(screen.getByRole("button", { name: /add user/i }));
    const addDialog = screen.getByRole("dialog");
    fireEvent.change(within(addDialog).getByLabelText(/name/i), { target: { value: "admin" } });
    fireEvent.click(within(addDialog).getByRole("button", { name: /preview/i }));

    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("Skip")).toBeInTheDocument();
    expect(within(dialog).getByText("Account already exists")).toBeInTheDocument();
  });

  it("previews then applies a per-user remove", async () => {
    apiMock.previewDeviceUsers.mockResolvedValue({
      writes_enabled: true,
      per_item: [{ name: "operator", action: "delete", reason: "Will be removed" }],
    });
    apiMock.deleteDeviceUsers.mockResolvedValue(okOutcome({ succeeded: ["operator"] }));

    renderSection(true, true, USERS);

    fireEvent.pointerDown(screen.getByRole("button", { name: /actions: operator/i }), { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /remove operator/i }));

    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("Delete")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: /confirm/i }));

    await waitFor(() =>
      expect(apiMock.deleteDeviceUsers).toHaveBeenCalledWith(TARGET_ID, ["operator"]),
    );
    expect(await within(dialog).findByText(/succeeded/i)).toBeInTheDocument();
  });

  it("shows the no-permission banner and blocks confirm when the caller may not write", async () => {
    apiMock.previewDeviceUsers.mockResolvedValue({ ...okPreview({}), writes_enabled: false });

    renderSection(true, true);

    fireEvent.click(screen.getByRole("button", { name: /add user/i }));
    const addDialog = screen.getByRole("dialog");
    fireEvent.change(within(addDialog).getByLabelText(/name/i), { target: { value: "cam-op" } });
    fireEvent.click(within(addDialog).getByRole("button", { name: /preview/i }));

    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(/no permission to change/i)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /confirm/i })).toBeDisabled();
    expect(apiMock.createDeviceUsers).not.toHaveBeenCalled();
  });

  it("shows the add form but hides remove controls when the caller may create but not delete", async () => {
    renderSection(true, false, USERS);

    // The account list still renders for everyone.
    expect(screen.getByText("operator")).toBeInTheDocument();
    // Create is permitted → the Add-user form (Preview button) is shown…
    expect(screen.getByRole("button", { name: /add user/i })).toBeInTheDocument();
    // …but delete is not, so no per-row Remove control appears.
    fireEvent.pointerDown(screen.getByRole("button", { name: /actions: operator/i }), { button: 0, ctrlKey: false });
    expect(await screen.findByRole("menuitem", { name: /edit operator/i })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /remove operator/i })).not.toBeInTheDocument();
  });

  it("shows remove controls but hides the add form when the caller may delete but not create", () => {
    renderSection(false, true, USERS);

    expect(screen.getByText("operator")).toBeInTheDocument();
    // Delete is permitted → the per-row Remove control is shown…
    expect(screen.getByRole("button", { name: /actions: operator/i })).toBeInTheDocument();
    // …but create is not, so the Add-user form (Preview button) is hidden.
    expect(screen.queryByRole("button", { name: /add user/i })).not.toBeInTheDocument();
  });

  it("hides the users section and skips its request without device_users", async () => {
    renderPage(false);

    await waitFor(() => expect(apiMock.getDeviceCapabilities).toHaveBeenCalled());
    expect(screen.queryByText("operator")).not.toBeInTheDocument();
    expect(apiMock.getDeviceUsers).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: /add user/i })).not.toBeInTheDocument();
  });

  it("shows the add form and remove controls for a manager", async () => {
    renderPage(true);

    expect(await screen.findByText("operator")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /add user/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /actions: operator/i })).toBeInTheDocument();
  });

  it("filters accounts by group or authority and distinguishes no matches from no users", () => {
    renderSection(false, false, USERS);

    const search = screen.getByRole("textbox", {
      name: "Search users, groups, or authorities",
    });
    fireEvent.change(search, { target: { value: "LIVE" } });

    expect(screen.getByText("operator")).toBeInTheDocument();
    expect(screen.queryByText("admin")).not.toBeInTheDocument();

    fireEvent.change(search, { target: { value: "missing" } });

    expect(screen.getByText("No users match your search.")).toBeInTheDocument();
    expect(screen.queryByText("No user accounts reported.")).not.toBeInTheDocument();
  });

  it("rejects a blank login name, then clears the error when the operator corrects it", async () => {
    apiMock.previewDeviceUsers.mockResolvedValue(okPreview({}));
    renderSection(true, true);

    fireEvent.click(screen.getByRole("button", { name: /add user/i }));
    const addDialog = screen.getByRole("dialog");
    fireEvent.click(within(addDialog).getByRole("button", { name: /preview/i }));

    expect(within(addDialog).getByRole("alert")).toHaveTextContent(
      "A login name is required.",
    );
    expect(apiMock.previewDeviceUsers).not.toHaveBeenCalled();

    fireEvent.change(within(addDialog).getByLabelText(/name/i), {
      target: { value: "cam-op" },
    });
    expect(within(addDialog).queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.click(within(addDialog).getByRole("button", { name: /preview/i }));

    await waitFor(() => expect(apiMock.previewDeviceUsers).toHaveBeenCalledTimes(1));
  });

  it("loads the authority catalog and sends a trimmed custom token in the preview", async () => {
    apiMock.getDeviceAuthorities.mockResolvedValue({ authorities: ["live", "all"] });
    apiMock.previewDeviceUsers.mockResolvedValue(okPreview({}));
    renderSection(true, true, [], true);

    fireEvent.click(screen.getByRole("button", { name: /add user/i }));
    const addDialog = screen.getByRole("dialog");
    await waitFor(() => {
      expect(apiMock.getDeviceAuthorities).toHaveBeenCalledWith(TARGET_ID);
    });
    fireEvent.change(within(addDialog).getByLabelText(/name/i), {
      target: { value: "cam-op" },
    });
    const customAuthority = within(addDialog).getByRole("textbox", {
      name: "Add an exact device token",
    });
    fireEvent.change(customAuthority, { target: { value: "  reboot  " } });
    fireEvent.click(within(addDialog).getByRole("button", { name: "Add" }));

    expect(customAuthority).toHaveValue("");
    fireEvent.click(within(addDialog).getByRole("button", { name: /preview/i }));

    await waitFor(() => {
      expect(apiMock.previewDeviceUsers).toHaveBeenCalledWith(TARGET_ID, {
        op: "create",
        users: [{ name: "cam-op", authorities: ["reboot"] }],
      });
    });
  });

  it("cancels account editing without sending an update and can reopen it", async () => {
    renderSection(true, true, USERS);

    fireEvent.pointerDown(screen.getByRole("button", { name: /actions: operator/i }), { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /edit operator/i }));
    const dialog = await screen.findByRole("dialog", { name: /edit operator/i });
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

    expect(screen.queryByRole("dialog", { name: /edit operator/i })).not.toBeInTheDocument();
    expect(apiMock.updateDeviceUsers).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByRole("button", { name: /actions: operator/i })).toHaveFocus());

    fireEvent.pointerDown(screen.getByRole("button", { name: /actions: operator/i }), { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /edit operator/i }));
    expect(await screen.findByRole("dialog", { name: /edit operator/i })).toBeInTheDocument();
  });

  it("cancels row removal after preview and restores the row menu focus", async () => {
    apiMock.previewDeviceUsers.mockResolvedValue(okPreview({ name: "operator", action: "delete" }));
    renderSection(true, true, USERS);
    const trigger = screen.getByRole("button", { name: /actions: operator/i });
    fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
    fireEvent.click(await screen.findByRole("menuitem", { name: /remove operator/i }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: /^cancel$/i }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(apiMock.deleteDeviceUsers).not.toHaveBeenCalled();
  });

  it("cancels a preview without applying and re-previews the same action", async () => {
    apiMock.previewDeviceUsers.mockResolvedValue(okPreview({}));
    renderSection(true, true);

    fireEvent.click(screen.getByRole("button", { name: /add user/i }));
    const addDialog = screen.getByRole("dialog");
    fireEvent.change(within(addDialog).getByLabelText(/name/i), {
      target: { value: "cam-op" },
    });
    fireEvent.click(within(addDialog).getByRole("button", { name: /preview/i }));

    const previewDialog = await screen.findByRole("dialog", { name: /add device user/i });
    fireEvent.click(within(previewDialog).getByRole("button", { name: "Cancel" }));

    expect(
      screen.queryByRole("dialog", { name: /add device user/i }),
    ).not.toBeInTheDocument();
    expect(apiMock.createDeviceUsers).not.toHaveBeenCalled();

    fireEvent.click(within(addDialog).getByRole("button", { name: /preview/i }));
    await waitFor(() => expect(apiMock.previewDeviceUsers).toHaveBeenCalledTimes(2));
  });

});
