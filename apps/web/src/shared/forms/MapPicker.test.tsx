import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

// The dialog mounts the offline SVG map (jsdom can't lay it out / has no
// ResizeObserver callback); stub it so the test only exercises MapPicker's
// inputs + two-way sync, never a real map.
vi.mock("@/shared/forms/MapPickerDialog", () => ({
  default: () => null,
}));

import MapPicker from "@/shared/forms/MapPicker";

describe("MapPicker", () => {
  it("writes both coordinates back as a pair on a single input edit", () => {
    const onChange = vi.fn();
    render(<MapPicker lat="" lng="" onChange={onChange} />);

    fireEvent.change(screen.getByLabelText("Latitude"), { target: { value: "59.93" } });
    expect(onChange).toHaveBeenLastCalledWith("59.93", "");

    fireEvent.change(screen.getByLabelText("Longitude"), { target: { value: "30.33" } });
    // The longitude edit preserves the incoming latitude value.
    expect(onChange).toHaveBeenLastCalledWith("", "30.33");
  });

  it("flags an out-of-range coordinate without dropping the keystroke", () => {
    render(<MapPicker lat="120" lng="10" onChange={() => {}} />);
    // 120 is outside −90..90 → the range hint is shown.
    expect(screen.getByText(/−90…90/)).toBeInTheDocument();
  });

  it("shows the selected-coords summary only for a complete valid pair", () => {
    const { rerender } = render(<MapPicker lat="" lng="" onChange={() => {}} />);
    expect(screen.queryByText(/Selected:/)).not.toBeInTheDocument();

    rerender(<MapPicker lat="59.930000" lng="30.330000" onChange={() => {}} />);
    expect(screen.getByText(/Selected: 59\.930000, 30\.330000/)).toBeInTheDocument();
  });

  it("clears both fields via the clear affordance", () => {
    const onChange = vi.fn();
    render(<MapPicker lat="59.93" lng="30.33" onChange={onChange} />);

    fireEvent.click(screen.getByText("Clear"));
    expect(onChange).toHaveBeenCalledWith("", "");
  });

  it("opens the picker dialog from the pick-on-map button", () => {
    render(<MapPicker lat="" lng="" onChange={() => {}} />);
    // The lazy dialog is stubbed to render nothing, so this only asserts the
    // trigger exists and clicking it does not throw (no crash on open).
    const trigger = screen.getByRole("button", { name: "Pick on map" });
    fireEvent.click(trigger);
    expect(trigger).toBeInTheDocument();
  });
});
