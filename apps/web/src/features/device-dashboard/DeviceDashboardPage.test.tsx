import { fireEvent, screen, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { MemoryRouter, Route, Routes } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import { ApiError } from "@/shared/api/client";
import { AllPermissions } from "@/test/permissions";
import DeviceDashboardPage from "@/features/device-dashboard/DeviceDashboardPage";
import { formatBytes } from "@/features/device-dashboard/format";
import type {
  DeviceDisk,
  DeviceInfo,
  DeviceRecording,
  DeviceUsers,
} from "@/features/device-dashboard/api";

const deviceApiMock = vi.hoisted(() => ({
  getDeviceInfo: vi.fn(),
  getDeviceUsers: vi.fn(),
  getDeviceDisk: vi.fn(),
  getDeviceRecording: vi.fn(),
  getDeviceCapabilities: vi.fn(),
  getDeviceChannels: vi.fn(),
}));
vi.mock("@/features/device-dashboard/api", () => deviceApiMock);

const connectionsApiMock = vi.hoisted(() => ({
  CONNECTION_TARGETS_QUERY_KEY: ["connections", "targets"] as const,
  getConnectionTarget: vi.fn(),
}));
vi.mock("@/features/connections/api", () => connectionsApiMock);

/** Capabilities with channel enumeration off, so CameraControl renders the
 * single default view and never issues a channel query in these tests. */
const CAPS_NO_CHANNELS = {
  info: true,
  users: true,
  disk: true,
  recording: true,
  snapshot: true,
  streaming: true,
  channels: false,
  ptz: false,
  export: false,
  authority_catalog: false,
  recording_search: false,
  user_update: true,
  permissions: {
    view: true,
    recordings: true,
    export: false,
    users: true,
    settings: true,
    ptz: false,
  },
};

const INFO: DeviceInfo = {
  model: "NVR-16",
  firmware: "V5.2.1 build 240118",
  serial: "NVR16-2400-000482",
  channels: 16,
  device_time: "2026-06-02T11:19:00Z",
};
const DISK: DeviceDisk = {
  disks: [
    { name: "HDD1", total_bytes: 4_000_000_000_000, used_bytes: 3_760_000_000_000, state: "ok", is_error: false },
    { name: "HDD2", total_bytes: 4_000_000_000_000, used_bytes: 1_240_000_000_000, state: "ok", is_error: false },
  ],
};
const USERS: DeviceUsers = {
  users: [
    { name: "admin", group: "Administrators", authorities: ["all", "playback"] },
    { name: "operator", group: "Operators", authorities: ["live"] },
  ],
};
const RECORDING: DeviceRecording = {
  active: true,
  oldest: "2026-05-03T11:20:00Z",
  newest: "2026-06-02T11:19:00Z",
  spans: [{ start: "2026-05-03T11:20:00Z", end: "2026-06-02T11:19:00Z" }],
};
const RECORDING_IDLE: DeviceRecording = { active: false, oldest: null, newest: null, spans: [] };
const DISK_EMPTY: DeviceDisk = { disks: [] };
const USERS_EMPTY: DeviceUsers = { users: [] };
const DISK_MIXED_STATES: DeviceDisk = {
  disks: [
    { name: "HDD-ERR", total_bytes: 1_000_000_000, used_bytes: 500_000_000, state: "error", is_error: true },
    { name: "HDD-RO", total_bytes: 1_000_000_000, used_bytes: 200_000_000, state: "read_only", is_error: false },
  ],
};

/** Bears a `status` below 500 so `retryOn5xx` does not schedule a retry, but is not an `ApiError`. */
class UnexpectedError extends Error {
  readonly status = 404;
}

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <MemoryRouter initialEntries={["/connections/devices/target-1"]}>
          <Routes>
            <Route path="/connections/devices/:targetId" element={<DeviceDashboardPage />} />
          </Routes>
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  );
}

describe("DeviceDashboardPage", () => {
  beforeEach(() => {
    deviceApiMock.getDeviceInfo.mockReset();
    deviceApiMock.getDeviceUsers.mockReset();
    deviceApiMock.getDeviceDisk.mockReset();
    deviceApiMock.getDeviceRecording.mockReset();
    deviceApiMock.getDeviceCapabilities.mockReset().mockResolvedValue(CAPS_NO_CHANNELS);
    deviceApiMock.getDeviceChannels.mockReset().mockResolvedValue({ channels: [] });
    connectionsApiMock.getConnectionTarget.mockReset().mockResolvedValue({
      id: "target-1",
      name: "North gate recorder",
      targetKind: "device",
      primaryIp: "198.51.100.24",
      family: "dvrip_sofia",
    });
  });

  it("shows the opened target name, address, and family in the page header", async () => {
    deviceApiMock.getDeviceInfo.mockResolvedValue(INFO);
    deviceApiMock.getDeviceUsers.mockResolvedValue(USERS);
    deviceApiMock.getDeviceDisk.mockResolvedValue(DISK);
    deviceApiMock.getDeviceRecording.mockResolvedValue(RECORDING);

    renderPage();

    expect(await screen.findByRole("heading", { name: "North gate recorder" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /198\.51\.100\.24/ }));
    expect(screen.getByText(/198\.51\.100\.24/)).toBeInTheDocument();
    expect(screen.getByText("dvrip_sofia")).toBeInTheDocument();
    expect(connectionsApiMock.getConnectionTarget).toHaveBeenCalledWith("target-1");
  });

  it("shows the existing safe failure component when target identity cannot be loaded", async () => {
    connectionsApiMock.getConnectionTarget.mockRejectedValue(
      new ApiError({
        status: 404,
        reasonCode: "CONNECTION_UNREACHABLE",
        message: "PRIVATE TARGET DETAIL",
      }),
    );

    renderPage();

    expect(await screen.findByText("Target unavailable")).toBeInTheDocument();
    expect(screen.queryByText("PRIVATE TARGET DETAIL")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(connectionsApiMock.getConnectionTarget).toHaveBeenCalledTimes(2);
  });

  it("explains itself when the device cannot be read at all", async () => {
    // An offline recorder, or one onboarded before its credentials were known,
    // answers `capabilities` with a 400. Every section is gated on that
    // response, so a silent failure leaves the operator a page title over empty
    // space — no tabs, no message, nothing to retry. On this fleet that is not
    // an edge case: a fifth of the recorders are unreachable at any time.
    deviceApiMock.getDeviceCapabilities.mockRejectedValue(
      new ApiError({
        status: 400,
        reasonCode: "CONNECTION_CREDENTIALS_REQUIRED",
        message: "Unable to connect to the device",
      }),
    );

    renderPage();

    // The shared failure notice names the cause and offers a way out.
    expect(await screen.findByRole("button", { name: /retry|повтор/i })).toBeInTheDocument();
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  });

  it("renders the device identity, a disk row, and a user row from mocked data", async () => {
    deviceApiMock.getDeviceInfo.mockResolvedValue(INFO);
    deviceApiMock.getDeviceUsers.mockResolvedValue(USERS);
    deviceApiMock.getDeviceDisk.mockResolvedValue(DISK);
    deviceApiMock.getDeviceRecording.mockResolvedValue(RECORDING);

    renderPage();

    // Model shows in both the header badge and the identity card.
    expect((await screen.findAllByText("NVR-16")).length).toBeGreaterThan(0);
    expect(screen.getByText("V5.2.1 build 240118")).toBeInTheDocument();
    // A disk row and a user row from mocked data.
    expect(screen.getByText("HDD1")).toBeInTheDocument();
    expect(screen.getByText("admin")).toBeInTheDocument();
    // DiskRow usage math: HDD1 is 3.76TB/4TB -> 94%, HDD2 1.24TB -> 31% (clamped, rounded).
    expect(screen.getByText("94%")).toBeInTheDocument();
    expect(screen.getByText("31%")).toBeInTheDocument();
  });

  it("renders a safe localized credentials failure instead of the backend message", async () => {
    const error = new ApiError({
      status: 400,
      reasonCode: "CONNECTION_CREDENTIALS_REQUIRED",
      message: "PRIVATE BACKEND CREDENTIAL DETAIL",
    });
    deviceApiMock.getDeviceInfo.mockRejectedValue(error);
    deviceApiMock.getDeviceUsers.mockRejectedValue(error);
    deviceApiMock.getDeviceDisk.mockRejectedValue(error);
    deviceApiMock.getDeviceRecording.mockRejectedValue(error);

    renderPage();

    expect(await screen.findByText("Connection is not configured")).toBeInTheDocument();
    expect(screen.queryByText("PRIVATE BACKEND CREDENTIAL DETAIL")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(deviceApiMock.getDeviceInfo).toHaveBeenCalledTimes(2);
  });

  it("formats byte sizes into human-readable units", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1024)).toBe("1 KB");
    expect(formatBytes(1_610_612_736)).toBe("1.5 GB");
  });

  it("shows an idle status and 'no footage retained' when the recorder has never recorded", async () => {
    deviceApiMock.getDeviceInfo.mockResolvedValue(INFO);
    deviceApiMock.getDeviceUsers.mockResolvedValue(USERS);
    deviceApiMock.getDeviceDisk.mockResolvedValue(DISK);
    deviceApiMock.getDeviceRecording.mockResolvedValue(RECORDING_IDLE);

    renderPage();

    expect(await screen.findByText("Idle")).toBeInTheDocument();
    expect(screen.getByText("No footage retained")).toBeInTheDocument();
    // The "Recorded spans" badge reflects the empty spans array.
    expect(screen.getByText("0")).toBeInTheDocument();
  });

  it("shows the empty-state copy when the device reports no disks and no user accounts", async () => {
    deviceApiMock.getDeviceInfo.mockResolvedValue(INFO);
    deviceApiMock.getDeviceUsers.mockResolvedValue(USERS_EMPTY);
    deviceApiMock.getDeviceDisk.mockResolvedValue(DISK_EMPTY);
    deviceApiMock.getDeviceRecording.mockResolvedValue(RECORDING);

    renderPage();

    expect(await screen.findByText("No storage devices reported.")).toBeInTheDocument();
    expect(screen.getByText("No user accounts reported.")).toBeInTheDocument();
  });

  it("labels an errored disk 'Error' and a read-only disk 'Read-only'", async () => {
    deviceApiMock.getDeviceInfo.mockResolvedValue(INFO);
    deviceApiMock.getDeviceUsers.mockResolvedValue(USERS);
    deviceApiMock.getDeviceDisk.mockResolvedValue(DISK_MIXED_STATES);
    deviceApiMock.getDeviceRecording.mockResolvedValue(RECORDING);

    renderPage();

    expect(await screen.findByText("HDD-ERR")).toBeInTheDocument();
    expect(screen.getByText("HDD-RO")).toBeInTheDocument();
    expect(screen.getByText("Error")).toBeInTheDocument();
    expect(screen.getByText("Read-only")).toBeInTheDocument();
  });

  it("keeps the dashboard grid when a single section fails with a 400 (op unsupported), rendering it muted", async () => {
    const unsupported = new ApiError({
      status: 400,
      message: "operation not supported by this device",
      reasonCode: "CONNECTION_UNSUPPORTED",
    });
    deviceApiMock.getDeviceInfo.mockResolvedValue(INFO);
    deviceApiMock.getDeviceUsers.mockResolvedValue(USERS);
    deviceApiMock.getDeviceDisk.mockResolvedValue(DISK);
    deviceApiMock.getDeviceRecording.mockRejectedValue(unsupported);

    renderPage();

    // The healthy sections still render their data…
    expect((await screen.findAllByText("NVR-16")).length).toBeGreaterThan(0);
    expect(screen.getByText("admin")).toBeInTheDocument();
    expect(screen.getByText("HDD1")).toBeInTheDocument();
    // …the unsupported section shows the muted placeholder, not the raw error…
    expect(screen.getByText("Not supported by this device")).toBeInTheDocument();
    // …and nothing hoists to the full-page credentials notice.
    expect(screen.queryByText(/details unavailable/i)).not.toBeInTheDocument();
  });

  it("shows a safe inline per-section error with retry for a genuine API failure", async () => {
    const notFound = new ApiError({
      status: 404,
      reasonCode: "CONNECTION_UNREACHABLE",
      message: "PRIVATE DEVICE DETAIL",
    });
    deviceApiMock.getDeviceInfo.mockResolvedValue(INFO);
    deviceApiMock.getDeviceUsers.mockResolvedValue(USERS);
    deviceApiMock.getDeviceDisk.mockRejectedValue(notFound);
    deviceApiMock.getDeviceRecording.mockResolvedValue(RECORDING);

    renderPage();

    const failureTitle = await screen.findByText("Target unavailable");
    expect(screen.queryByText("PRIVATE DEVICE DETAIL")).not.toBeInTheDocument();
    expect(screen.queryByText(/details unavailable/i)).not.toBeInTheDocument();
    expect(screen.getByText("admin")).toBeInTheDocument();
    const failureRegion = failureTitle.closest('[role="alert"]');
    expect(failureRegion).not.toBeNull();
    fireEvent.click(within(failureRegion as HTMLElement).getByRole("button", { name: "Retry" }));
    expect(deviceApiMock.getDeviceDisk).toHaveBeenCalledTimes(2);
  });

  it("falls back to a generic message when a section fails with a non-ApiError", async () => {
    deviceApiMock.getDeviceInfo.mockResolvedValue(INFO);
    deviceApiMock.getDeviceUsers.mockResolvedValue(USERS);
    deviceApiMock.getDeviceDisk.mockResolvedValue(DISK);
    deviceApiMock.getDeviceRecording.mockRejectedValue(new UnexpectedError("boom"));

    renderPage();

    expect(await screen.findByText("Connection failed")).toBeInTheDocument();
    expect(screen.queryByText(/details unavailable/i)).not.toBeInTheDocument();
  });
});
