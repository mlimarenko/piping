import { cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import {
  removeObservationHandoff,
  writeObservationHandoff,
} from "@/features/connections/observation";
import ObservedSession from "./ObservedSession";

vi.mock("@/features/connections/components/connections/LiveSessionViewer", () => ({
  default: (props: { readOnly?: boolean; websocketUrl: string }) => (
    <div data-testid="observer-viewer">
      {String(props.readOnly)} {props.websocketUrl}
    </div>
  ),
}));

const sessionId = "session-id";

function show(path = `/connections/observe/${sessionId}`) {
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/connections/observe/:sessionId" element={<ObservedSession />} />
        <Route path="/connections/active" element={<div>active connections</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  removeObservationHandoff(sessionId);
});

afterEach(() => {
  cleanup();
  removeObservationHandoff(sessionId);
});

describe("separate observation window", () => {
  it("renders the read-only stream from the stored handoff", () => {
    writeObservationHandoff({
      sessionId,
      hostId: "target-id",
      hostName: "Target",
      userLabel: "Operator",
      protocol: "vnc",
      websocketUrl: `ws://localhost:3000/api/v1/ws/observe/${sessionId}`,
    });
    show();

    expect(screen.getByTestId("observer-viewer")).toHaveTextContent(
      `true ws://localhost:3000/api/v1/ws/observe/${sessionId}`,
    );
    expect(screen.getByRole("heading", { name: "Target" })).toBeInTheDocument();
    expect(screen.getByText("Operator")).toBeInTheDocument();
    expect(screen.getAllByText("View only")).not.toHaveLength(0);
  });

  it("explains a window opened without observation data", () => {
    show();

    expect(screen.queryByTestId("observer-viewer")).not.toBeInTheDocument();
    expect(screen.getByText(/no observation data/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Back to active connections/ })).toHaveAttribute(
      "href",
      "/connections/active",
    );
  });
});
