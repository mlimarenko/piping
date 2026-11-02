import { fireEvent, render, screen } from "@testing-library/react";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { ConnectionTarget } from "@/shared/api/types";
import { testUuid } from "@/test/ids";
import ConnectButtons from "./ConnectButtons";

type LaunchOptionsHookResult = {
  data?: unknown;
  dataUpdatedAt: number;
  isPending: boolean;
  isError: boolean;
};

const useConnectionLaunchOptionsMock = vi.fn<
  (hostId: string) => LaunchOptionsHookResult
>();

vi.mock("@/features/connections/hooks", () => ({
  useConnectionLaunchOptions: (hostId: string) => useConnectionLaunchOptionsMock(hostId),
}));

// These tests exercise launch behaviour for a user who can connect; grant
// CONNECT so the permission gate is a no-op here. The gating itself is covered
// by the dedicated "hides … without CONNECT" cases below.
const canMock = vi.fn<(action: string, resourceType: string) => boolean>(() => true);

vi.mock("@/shared/auth/permissionsContext", () => ({
  usePermissions: () => ({
    roles: [],
    isAdmin: false,
    can: (action: string, resourceType: string) => canMock(action, resourceType),
    canAny: () => false,
    hasRole: () => false,
  }),
}));

const host: ConnectionTarget = {
  id: testUuid("host-1"),
  name: "ares",
  targetKind: "virtual_machine",
  status: "active",
};

describe("ConnectButtons", () => {
  beforeEach(() => {
    useConnectionLaunchOptionsMock.mockReset();
    canMock.mockReset();
    canMock.mockReturnValue(true);
  });

  it("renders no connect buttons when the user lacks CONNECT on CONNECTION_TARGET", () => {
    canMock.mockImplementation(
      (action, resourceType) => !(action === "CONNECT" && resourceType === "CONNECTION_TARGET"),
    );
    useConnectionLaunchOptionsMock.mockReturnValue({
      data: {
        hostId: host.id,
        protocols: [
          {
            protocol: "ssh",
            render: "buttons",
            accounts: [
              {
                entryId: "config:connconf:ssh:0",
                kind: "shared",
                label: "root",
                username: "root",
                source: { recordName: "Cashier — SSH" },
                isDefault: false,
                requiresInput: false,
              },
            ],
          },
        ],
      },
      dataUpdatedAt: Date.now(),
      isPending: false,
      isError: false,
    });

    render(<ConnectButtons host={host} onConnect={vi.fn()} />);

    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("opens the ad-hoc credentials dialog (fill-in) when a shared RDP account still requires input", () => {
    const onConnect = vi.fn();
    const onOpenAdHoc = vi.fn();
    useConnectionLaunchOptionsMock.mockReturnValue({
      data: {
        hostId: host.id,
        protocols: [
          {
            protocol: "rdp",
            render: "buttons",
            accounts: [
              {
                entryId: "config:connconf:rdp:0",
                kind: "shared",
                label: "Сервер — RDP",
                source: { recordName: "Windows RDP" },
                isDefault: false,
                requiresInput: true,
              },
              {
                entryId: "user:connconf:rdp:0:grad",
                kind: "personal",
                label: "GRAD",
                username: "m.limarenko@grad.net",
                source: { credentialSetName: "GRAD" },
                isDefault: true,
                requiresInput: true,
              },
            ],
          },
        ],
      },
      dataUpdatedAt: Date.now(),
      isPending: false,
      isError: false,
    });

    render(<ConnectButtons host={host} onConnect={onConnect} onOpenAdHoc={onOpenAdHoc} />);

    // Personal RDP credentials never render as standalone buttons (the overlay
    // rule folds them into the shared action).
    expect(screen.queryByRole("button", { name: /GRAD/i })).not.toBeInTheDocument();
    const button = screen.getByRole("button", { name: /Сервер — RDP/i });
    // The button is now actionable: it opens the fill-in dialog instead of being
    // disabled.
    expect(button).toBeEnabled();

    fireEvent.click(button);

    // Opening the dialog launches no session directly; it routes through the
    // ad-hoc handler in fill-in mode with the selected account.
    expect(onConnect).not.toHaveBeenCalled();
    // The RDP overlay rule backs the shared action with the selected personal
    // entry when it still requires input, so the fill-in dialog targets that
    // personal entry (carrying its username) rather than the bare shared one.
    expect(onOpenAdHoc).toHaveBeenCalledWith(
      expect.objectContaining({
        protocol: "rdp",
        host: expect.objectContaining({ id: host.id }),
        account: expect.objectContaining({
          entryId: "user:connconf:rdp:0:grad",
          username: "m.limarenko@grad.net",
        }),
      }),
    );
  });

  it("renders nothing for a protocol with no launch-ready accounts", () => {
    const onConnect = vi.fn();
    const onOpenAdHoc = vi.fn();
    // The launch-options endpoint emits an entry for every protocol regardless of
    // config; an empty `accounts` list means no connection-config applies, which
    // the backend connect path requires, so the UI shows nothing — even with an
    // ad-hoc handler available.
    useConnectionLaunchOptionsMock.mockReturnValue({
      data: {
        hostId: host.id,
        protocols: [{ protocol: "ssh", render: "buttons", accounts: [] }],
      },
      dataUpdatedAt: Date.now(),
      isPending: false,
      isError: false,
    });

    render(<ConnectButtons host={host} onConnect={onConnect} onOpenAdHoc={onOpenAdHoc} />);

    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(onOpenAdHoc).not.toHaveBeenCalled();
  });

  it("renders shared accounts as direct buttons and personal accounts in a caret dropdown", () => {
    const onConnect = vi.fn();
    useConnectionLaunchOptionsMock.mockReturnValue({
      data: {
        hostId: host.id,
        protocols: [
          {
            protocol: "ssh",
            render: "buttons",
            accounts: [
              {
                entryId: "config:connconf:ssh:0",
                kind: "shared",
                label: "root",
                username: "root",
                source: { recordName: "Cashier — SSH" },
                isDefault: false,
                requiresInput: false,
              },
              {
                entryId: "user:connconf:ssh:0:me",
                kind: "personal",
                label: "me",
                username: "ops",
                source: { credentialSetName: "My SSH" },
                isDefault: true,
                requiresInput: false,
              },
            ],
          },
        ],
      },
      dataUpdatedAt: Date.now(),
      isPending: false,
      isError: false,
    });

    render(<ConnectButtons host={host} onConnect={onConnect} />);

    // Shared account is a direct button…
    const sharedButton = screen.getByRole("button", { name: /^SSH root$/i });
    fireEvent.click(sharedButton);
    expect(onConnect).toHaveBeenCalledWith(host, "ssh", "config:connconf:ssh:0", "root");

    // …the personal account is not a direct button; it lives behind the caret.
    expect(screen.queryByRole("button", { name: /^me$/i })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Personal/i })).toBeInTheDocument();
  });

  it("launches the configured RDP server action through the default personal credential set", () => {
    const onConnect = vi.fn();
    useConnectionLaunchOptionsMock.mockReturnValue({
      data: {
        hostId: host.id,
        protocols: [
          {
            protocol: "rdp",
            render: "buttons",
            accounts: [
              {
                entryId: "config:connconf:rdp:0",
                kind: "shared",
                label: "Сервер — RDP",
                source: { recordName: "Windows RDP" },
                isDefault: false,
                requiresInput: true,
              },
              {
                entryId: "user:connconf:rdp:0:grad",
                kind: "personal",
                label: "GRAD",
                username: "m.limarenko@grad.net",
                source: { credentialSetName: "GRAD" },
                isDefault: true,
                requiresInput: false,
              },
              {
                entryId: "user:connconf:rdp:0:ops",
                kind: "personal",
                label: "OPS",
                username: "ops",
                source: { credentialSetName: "OPS" },
                isDefault: false,
                requiresInput: false,
              },
            ],
          },
        ],
      },
      dataUpdatedAt: Date.now(),
      isPending: false,
      isError: false,
    });

    render(<ConnectButtons host={host} onConnect={onConnect} />);

    expect(screen.queryByRole("button", { name: /GRAD/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /OPS/i })).not.toBeInTheDocument();
    const button = screen.getByRole("button", { name: /Сервер — RDP/i });
    expect(button).toBeEnabled();
    expect(button).toHaveAttribute("title", expect.stringContaining("m.limarenko@grad.net"));

    fireEvent.click(button);

    expect(onConnect).toHaveBeenCalledWith(
      host,
      "rdp",
      "user:connconf:rdp:0:grad",
      "m.limarenko@grad.net",
    );
  });

  it("keeps launch-ready shared RDP credentials instead of replacing them with personal credentials", () => {
    const onConnect = vi.fn();
    useConnectionLaunchOptionsMock.mockReturnValue({
      data: {
        hostId: host.id,
        protocols: [
          {
            protocol: "rdp",
            render: "buttons",
            accounts: [
              {
                entryId: "config:connconf:rdp:0",
                kind: "shared",
                label: "Сервер — RDP",
                username: "leader",
                source: { recordName: "Linux RDP" },
                isDefault: false,
                requiresInput: false,
              },
              {
                entryId: "user:connconf:rdp:0:grad",
                kind: "personal",
                label: "GRAD",
                username: "m.limarenko@grad.net",
                source: { credentialSetName: "GRAD" },
                isDefault: true,
                requiresInput: false,
              },
            ],
          },
        ],
      },
      dataUpdatedAt: Date.now(),
      isPending: false,
      isError: false,
    });

    render(<ConnectButtons host={host} onConnect={onConnect} />);

    expect(screen.queryByRole("button", { name: /GRAD/i })).not.toBeInTheDocument();
    const button = screen.getByRole("button", { name: /Сервер — RDP/i });
    expect(button).toBeEnabled();
    expect(button).toHaveAttribute("title", expect.stringContaining("leader"));
    expect(button).not.toHaveAttribute("title", expect.stringContaining("m.limarenko"));

    fireEvent.click(button);

    expect(onConnect).toHaveBeenCalledWith(
      host,
      "rdp",
      "config:connconf:rdp:0",
      "leader",
    );
  });

  it("shows the neutral Accounts entry for a non-video family and opens the device console", () => {
    const onConnect = vi.fn();
    const linuxHost: ConnectionTarget = {
      id: testUuid("host-linux"),
      name: "web-prod-01",
      targetKind: "device",
      status: "active",
      family: "linux",
      isCctv: false,
    };
    useConnectionLaunchOptionsMock.mockReturnValue({
      data: {
        hostId: linuxHost.id,
        protocols: [
          {
            protocol: "ssh",
            render: "buttons",
            accounts: [
              {
                entryId: "config:connconf:ssh:0",
                kind: "shared",
                label: "root",
                username: "root",
                source: { recordName: "Linux Server — SSH" },
                isDefault: false,
                requiresInput: false,
              },
            ],
          },
        ],
      },
      dataUpdatedAt: Date.now(),
      isPending: false,
      isError: false,
    });

    render(<ConnectButtons host={linuxHost} onConnect={onConnect} />);

    const accounts = screen.getByRole("button", { name: /^Accounts$/i });
    fireEvent.click(accounts);
    expect(onConnect).toHaveBeenCalledWith(linuxHost, "cctv");
  });

  it("keeps only the CCTV entry for a video device, with no neutral Accounts button", () => {
    const onConnect = vi.fn();
    const cctvHost: ConnectionTarget = {
      id: testUuid("host-cctv"),
      name: "nvr-ams-01",
      targetKind: "device",
      status: "active",
      family: "dvrip_sofia",
      isCctv: true,
    };
    useConnectionLaunchOptionsMock.mockReturnValue({
      data: {
        hostId: cctvHost.id,
        protocols: [
          {
            protocol: "cctv",
            render: "buttons",
            accounts: [
              {
                entryId: "config:connconf:cctv:0",
                kind: "shared",
                label: "CCTV",
                source: { recordName: "CCTV Recorder · admin" },
                isDefault: false,
                requiresInput: false,
              },
            ],
          },
        ],
      },
      dataUpdatedAt: Date.now(),
      isPending: false,
      isError: false,
    });

    render(<ConnectButtons host={cctvHost} onConnect={onConnect} />);

    expect(screen.queryByRole("button", { name: /^Accounts$/i })).not.toBeInTheDocument();
    const cctv = screen.getByRole("button", { name: /CCTV/ });
    fireEvent.click(cctv);
    expect(onConnect).toHaveBeenCalledWith(cctvHost, "cctv", "config:connconf:cctv:0", undefined);
  });
});
