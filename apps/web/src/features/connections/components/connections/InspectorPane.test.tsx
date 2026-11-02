import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import InspectorPane from "./InspectorPane";

describe("target details", () => {
  it("makes automatic floating explicit on narrow screens instead of offering an ineffective pin action", () => {
    const onPin = vi.fn();
    render(<InspectorPane width={320} onResize={vi.fn()} pinned onPin={onPin} narrow><p>Selected host</p></InspectorPane>);
    const pin = screen.getByRole("button", { name: "On narrow screens, details float over sessions" });
    expect(pin).toBeDisabled();
    fireEvent.click(pin);
    expect(onPin).not.toHaveBeenCalled();
    expect(screen.getByText("Selected host")).toBeInTheDocument();
  });

  it("allows pinning at sufficient width without replacing the details content", () => {
    const onPin = vi.fn();
    const view = render(<InspectorPane width={320} onResize={vi.fn()} pinned={false} onPin={onPin} narrow={false}><input aria-label="Details draft" defaultValue="retained" /></InspectorPane>);
    const input = screen.getByRole("textbox", { name: "Details draft" });
    fireEvent.click(screen.getByRole("button", { name: "Pin details" }));
    expect(onPin).toHaveBeenCalledTimes(1);
    view.rerender(<InspectorPane width={320} onResize={vi.fn()} pinned onPin={onPin} narrow={false}><input aria-label="Details draft" defaultValue="retained" /></InspectorPane>);
    expect(screen.getByRole("textbox", { name: "Details draft" })).toBe(input);
    expect(screen.getByRole("button", { name: "Unpin details" })).toBeEnabled();
  });
});
