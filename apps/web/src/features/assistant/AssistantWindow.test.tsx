import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import { AssistantWindow } from "./AssistantWindow";

const assistant = vi.hoisted(() => ({
  window: { open: true, dock: "right", width: 440, height: 600 },
  assistantReady: true,
  sessions: [{ id: "long-session", title: "Я".repeat(200), lifecycle: "active", message_count: 12, status: "idle" }],
  selectedSessionId: null,
  selectedSession: null,
  selectSession: vi.fn(),
  setWindowSize: vi.fn(),
  setDock: vi.fn(),
  setOpen: vi.fn(),
  refresh: vi.fn(),
  canUseSessionShell: false,
}));
vi.mock("./assistantContext", () => ({ useAssistant: () => assistant }));

describe("Assistant session selectors", () => {
  it("contains accepted long titles and status text within the selector while retaining selection", () => {
    render(<AllPermissions><AssistantWindow /></AllPermissions>);
    const title = screen.getByText("Я".repeat(200));
    const status = screen.getByText("12 · idle");
    expect(title).toHaveClass("w-full", "min-w-0", "truncate");
    expect(status).toHaveClass("w-full", "min-w-0", "truncate");
    fireEvent.click(title);
    expect(assistant.selectSession).toHaveBeenCalledWith("long-session");
  });
});
