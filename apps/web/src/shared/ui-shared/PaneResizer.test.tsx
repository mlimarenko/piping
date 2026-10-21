import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import PaneResizer from "./PaneResizer";

describe("panel resizing", () => {
  it("changes the trailing inspector size with keyboard and pointer, inside its bounds", () => {
    function Fixture() {
      const [width, setWidth] = useState(320);
      return <><PaneResizer orientation="horizontal" reverse size={width} min={280} max={480} label="Details width" onResize={setWidth} /><output aria-label="Width">{width}</output></>;
    }
    render(<Fixture />);
    const handle = screen.getByRole("slider", { name: "Details width" });
    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(handle).toHaveAttribute("aria-valuenow", "340");
    fireEvent.mouseDown(handle, { clientX: 100 });
    fireEvent.mouseMove(document, { clientX: 60 });
    fireEvent.mouseUp(document);
    expect(handle).toHaveAttribute("aria-valuenow", "380");
    fireEvent.keyDown(handle, { key: "Home" });
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(handle).toHaveAttribute("aria-valuenow", "280");
    fireEvent.keyDown(handle, { key: "End" });
    expect(handle).toHaveAttribute("aria-valuenow", "480");
  });
});
