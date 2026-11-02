import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import CameraControl from "@/features/device-dashboard/CameraControl";
import type { DeviceCapabilities } from "@/features/device-dashboard/api";
import type * as DeviceApi from "@/features/device-dashboard/api";

const apiMock = vi.hoisted(() => ({ getDeviceChannels: vi.fn() }));
vi.mock("@/features/device-dashboard/api", async (importOriginal) => {
  const actual = await importOriginal<typeof DeviceApi>();
  return { ...actual, getDeviceChannels: apiMock.getDeviceChannels };
});

const CAPS = (channels: boolean): DeviceCapabilities => ({
  info: true,
  users: false,
  disk: false,
  recording: false,
  snapshot: true,
  streaming: true,
  channels,
  ptz: false,
  export: false,
  authority_catalog: false,
  recording_search: false,
  user_update: false,
  permissions: {
    view: true,
    recordings: false,
    export: false,
    users: false,
    settings: false,
    ptz: false,
  },
});

function renderControl(props: Parameters<typeof CameraControl>[0]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <CameraControl {...props} />
    </QueryClientProvider>,
  );
}

function requestUrl(input: string | URL | Request): string {
  return input instanceof Request ? input.url : input.toString();
}

/** The current live-stream request URL. */
function liveRequestUrl(): string {
  const calls = vi.mocked(fetch).mock.calls;
  const call = [...calls].reverse().find(([input]) => requestUrl(input).includes("/stream"));
  return call ? requestUrl(call[0]) : "";
}

describe("CameraControl", () => {
  beforeEach(() => {
    apiMock.getDeviceChannels.mockReset();
    const encoder = new TextEncoder();
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation((input: string | URL | Request) => {
        const url = requestUrl(input);
        if (url.includes("/snapshot")) {
          return Promise.resolve(new Response(new Blob(["jpeg"], { type: "image/jpeg" })));
        }
        const body = new ReadableStream<Uint8Array>({
          start(controller) {
            controller.enqueue(
              encoder.encode(
                "--camera\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\njpeg\r\n",
              ),
            );
          },
        });
        return Promise.resolve(
          new Response(body, {
            headers: { "Content-Type": "multipart/x-mixed-replace; boundary=camera" },
          }),
        );
      }),
    );
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: vi.fn(() => "blob:test-frame"),
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: vi.fn(),
    });
    // View-mode settings persist per target; isolate tests from each other.
    window.localStorage.clear();
  });

  it("renders a single default view (no channel param) when the family has no channel enumeration", async () => {
    renderControl({ targetId: "t1", capabilities: CAPS(false) });
    // The default view omits the channel selector and the channel query; a
    // streaming-capable family opens the live stream, not stills.
    await waitFor(() => expect(liveRequestUrl()).toContain("/api/v1/cctv/t1/stream"));
    expect(liveRequestUrl()).not.toContain("channel=");
    expect(screen.queryByRole("group", { name: /channels/i })).not.toBeInTheDocument();
    expect(apiMock.getDeviceChannels).not.toHaveBeenCalled();
  });

  it("renders a channel selector from the device list and points the picture at the picked channel", async () => {
    apiMock.getDeviceChannels.mockResolvedValue({
      channels: [
        {
          index: 1,
          name: "Entrance",
          enabled: null,
          ptz_support: "unsupported",
          recording: null,
        },
        {
          index: 2,
          name: "Register",
          enabled: null,
          ptz_support: "unknown",
          recording: null,
        },
      ],
    });
    renderControl({ targetId: "t9", capabilities: CAPS(true) });

    // The first channel is selected and drives the picture.
    const register = await screen.findByRole("button", { name: /Register/, pressed: false });
    expect(screen.getByRole("button", { name: /Entrance/, pressed: true })).toBeInTheDocument();
    await waitFor(() => expect(liveRequestUrl()).toContain("channel=1"));

    // Selecting channel 2 repoints the picture at that camera.
    fireEvent.click(register);
    await waitFor(() => expect(liveRequestUrl()).toContain("channel=2"));
  });

  it("synthesizes numbered channels from the info count when the device lists none", async () => {
    apiMock.getDeviceChannels.mockResolvedValue({ channels: [] });
    renderControl({ targetId: "t3", capabilities: CAPS(true), infoChannelCount: 4 });

    // Four numbered choices, no names, from the info readout (locale-agnostic:
    // assert the count, not the localized "Ch"/"Кан" label).
    await waitFor(() => expect(within(screen.getByRole("group", { name: /channels/i })).getAllByRole("button")).toHaveLength(4));
    await waitFor(() => expect(liveRequestUrl()).toContain("channel=1"));
  });

  it("falls back to stills for a family without streaming, and the toggle is hidden", async () => {
    const caps = { ...CAPS(false), streaming: false };
    renderControl({ targetId: "t5", capabilities: caps });
    await screen.findByRole("img", { name: /snapshot/i });
    expect(vi.mocked(fetch).mock.calls.some(([input]) => requestUrl(input).includes("/api/v1/cctv/t5/snapshot"))).toBe(true);
    expect(screen.queryByRole("button", { name: /^(Stream|Поток)$/ })).not.toBeInTheDocument();
  });

  it("switches to stills when the source toggle is clicked and persists the choice", async () => {
    renderControl({ targetId: "t6", capabilities: CAPS(false) });
    await screen.findByRole("img", { name: /live video/i });
    fireEvent.click(screen.getByRole("button", { name: /^(Stream|Поток)$/ }));
    await screen.findByRole("img", { name: /snapshot/i });
    expect(vi.mocked(fetch).mock.calls.some(([input]) => requestUrl(input).includes("/snapshot"))).toBe(true);
    expect(window.localStorage.getItem("cctv:live:t6")).toContain('"stills"');
  });

  it("shows PTZ only for the selected channel with confirmed support", async () => {
    const caps: DeviceCapabilities = {
      ...CAPS(true),
      ptz: true,
      permissions: {
        view: true,
        recordings: false,
        export: false,
        users: false,
        settings: false,
        ptz: true,
      },
    };
    apiMock.getDeviceChannels.mockResolvedValue({
      channels: [
        {
          index: 1,
          name: "Fixed",
          enabled: true,
          ptz_support: "unsupported",
          recording: null,
        },
        {
          index: 2,
          name: "Movable",
          enabled: true,
          ptz_support: "supported",
          recording: null,
        },
        {
          index: 3,
          name: "Unverified",
          enabled: true,
          ptz_support: "unknown",
          recording: null,
        },
      ],
    });
    renderControl({ targetId: "t7", capabilities: caps });

    const movable = await screen.findByRole("button", { name: /Movable/ });
    expect(screen.queryByRole("button", { name: /Up/i })).not.toBeInTheDocument();

    fireEvent.click(movable);
    expect(await screen.findByRole("button", { name: /Up/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Unverified/ }));
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: /Up/i })).not.toBeInTheDocument(),
    );
  });

  it("hides the PTZ pad entirely (no disabled joystick) when the caller lacks PTZ permission", async () => {
    const caps: DeviceCapabilities = {
      ...CAPS(false),
      ptz: true,
      permissions: {
    view: true,
    recordings: false,
    export: false,
    users: false,
    settings: false,
    ptz: false,
  },
    };
    renderControl({ targetId: "t8", capabilities: caps });
    // Let the view settle, then assert the pad is absent — not merely disabled.
    await screen.findByRole("img", { name: /live video/i });
    expect(screen.queryByRole("button", { name: /Up/i })).not.toBeInTheDocument();
  });
});
