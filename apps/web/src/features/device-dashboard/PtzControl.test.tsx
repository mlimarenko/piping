import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import PtzControl from "@/features/device-dashboard/PtzControl";
import type * as DeviceApi from "@/features/device-dashboard/api";

const apiMock = vi.hoisted(() => ({ sendPtzCommand: vi.fn() }));
vi.mock("@/features/device-dashboard/api", async (importOriginal) => {
  const actual = await importOriginal<typeof DeviceApi>();
  return { ...actual, sendPtzCommand: apiMock.sendPtzCommand };
});

describe("PtzControl", () => {
  beforeEach(() => apiMock.sendPtzCommand.mockReset().mockResolvedValue(undefined));

  it("uses opaque compact controls with visible keyboard focus", () => {
    render(<PtzControl targetId="cam-1" channel={3} />);
    for (const button of screen.getAllByRole("button")) {
      expect(button).toHaveClass("h-8", "w-8", "bg-background", "border-border", "focus-visible:ring-2");
    }
    expect(apiMock.sendPtzCommand).not.toHaveBeenCalled();
  });

  it("orders stop after an in-flight move when release is immediate", async () => {
    let resolveMove: (() => void) | undefined;
    apiMock.sendPtzCommand.mockImplementation(
      (_target: string, command?: { kind: string }) => {
        if (command?.kind === "move") {
          return new Promise<void>((resolve) => {
            resolveMove = resolve;
          });
        }
        return Promise.resolve();
      },
    );
    render(<PtzControl targetId="cam-1" channel={3} />);
    const right = screen.getByRole("button", { name: /Right/i });

    fireEvent.pointerDown(right);
    await waitFor(() => expect(apiMock.sendPtzCommand).toHaveBeenCalledTimes(1));
    fireEvent.pointerUp(right);
    expect(apiMock.sendPtzCommand).toHaveBeenCalledTimes(1);

    resolveMove?.();
    await waitFor(() =>
      expect(apiMock.sendPtzCommand).toHaveBeenNthCalledWith(2, "cam-1", {
        kind: "stop",
        channel: 3,
      }),
    );
  });

  it("sends a continuous move on press and a stop on release", async () => {
    render(<PtzControl targetId="cam-1" channel={3} />);
    const right = screen.getByRole("button", { name: /Right/i });

    fireEvent.pointerDown(right);
    await waitFor(() =>
      expect(apiMock.sendPtzCommand).toHaveBeenCalledWith("cam-1", {
        kind: "move",
        channel: 3,
        pan: 1,
        tilt: 0,
        zoom: 0,
      }),
    );

    fireEvent.pointerUp(right);
    await waitFor(() =>
      expect(apiMock.sendPtzCommand).toHaveBeenCalledWith("cam-1", { kind: "stop", channel: 3 }),
    );
  });

  it("issues a zoom-in move with a positive zoom velocity", async () => {
    render(<PtzControl targetId="cam-2" channel={1} />);
    fireEvent.pointerDown(screen.getByRole("button", { name: /Zoom in/i }));
    await waitFor(() =>
      expect(apiMock.sendPtzCommand).toHaveBeenCalledWith("cam-2", {
        kind: "move",
        channel: 1,
        pan: 0,
        tilt: 0,
        zoom: 1,
      }),
    );
  });
});
