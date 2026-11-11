import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { HoverCard, HoverCardContent, HoverCardTrigger } from "./hover-card";

function TreeRowMetrics({ expanded = true }: { expanded?: boolean }) {
  return (
    <div data-testid="tree-scroll" style={{ overflow: "auto", contain: "content" }}>
      {expanded && (
        <div style={{ transform: "translateY(80px)", contain: "layout paint" }}>
          <HoverCard openDelay={0} closeDelay={0}>
            <HoverCardTrigger asChild>
              <button type="button">Host metrics</button>
            </HoverCardTrigger>
            <HoverCardContent>Memory usage: 42%</HoverCardContent>
          </HoverCard>
        </div>
      )}
      <div>Following host</div>
    </div>
  );
}

describe("HoverCard in a contained tree row", () => {
  it("keeps hover information outside the paint-contained scroll viewport", async () => {
    const { rerender } = render(<TreeRowMetrics />);
    fireEvent.pointerEnter(screen.getByRole("button", { name: "Host metrics" }), { pointerType: "mouse" });

    const content = await screen.findByText("Memory usage: 42%");
    expect(screen.getByTestId("tree-scroll")).not.toContainElement(content);

    rerender(<TreeRowMetrics expanded={false} />);
    expect(screen.queryByText("Memory usage: 42%")).not.toBeInTheDocument();
  });

  it("preserves focus opening and blur dismissal", async () => {
    render(<TreeRowMetrics />);
    const trigger = screen.getByRole("button", { name: "Host metrics" });
    fireEvent.focus(trigger);
    expect(await screen.findByText("Memory usage: 42%")).toBeInTheDocument();

    fireEvent.blur(trigger);
    await waitFor(() => expect(screen.queryByText("Memory usage: 42%")).not.toBeInTheDocument());
  });
});
