import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./tooltip";

function TreeRowTooltip({ expanded = true }: { expanded?: boolean }) {
  return (
    <TooltipProvider delayDuration={0}>
      <div data-testid="tree-scroll" style={{ overflow: "auto", contain: "content" }}>
        {expanded && (
          <div data-testid="tree-row" style={{ transform: "translateY(80px)", contain: "layout paint" }}>
            <Tooltip>
              <TooltipTrigger asChild>
                <button type="button">Recorder</button>
              </TooltipTrigger>
              <TooltipContent side="right">Recorder details</TooltipContent>
            </Tooltip>
          </div>
        )}
        <div>Following host</div>
      </div>
    </TooltipProvider>
  );
}

describe("Tooltip in a contained tree row", () => {
  it("opens outside the row and scroll viewport on hover", async () => {
    render(<TreeRowTooltip />);
    fireEvent.pointerMove(screen.getByRole("button", { name: "Recorder" }), { pointerType: "mouse" });

    const tooltip = await screen.findByRole("tooltip");
    expect(tooltip).toHaveTextContent("Recorder details");
    expect(screen.getByTestId("tree-scroll")).not.toContainElement(tooltip);
    expect(screen.getByTestId("tree-row")).not.toContainElement(tooltip);

    fireEvent.scroll(screen.getByTestId("tree-scroll"));
    await waitFor(() => expect(screen.queryByRole("tooltip")).not.toBeInTheDocument());
  });

  it("opens from keyboard focus, closes with Escape, and unmounts on collapse", async () => {
    const { rerender } = render(<TreeRowTooltip />);
    const trigger = screen.getByRole("button", { name: "Recorder" });
    fireEvent.focus(trigger);
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Recorder details");

    fireEvent.keyDown(trigger, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("tooltip")).not.toBeInTheDocument());

    fireEvent.blur(trigger);
    fireEvent.focus(trigger);
    expect(await screen.findByRole("tooltip")).toBeInTheDocument();
    rerender(<TreeRowTooltip expanded={false} />);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });
});
