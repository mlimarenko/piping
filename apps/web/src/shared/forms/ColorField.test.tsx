import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import ColorField from "./ColorField";
import { isValidHex, normalizeHex, toColorInputValue } from "./colorUtils";

/** Controlled harness mirroring how EntityFormBody wires the field. */
function Harness({
  initial = "",
  onChange,
}: {
  initial?: string;
  onChange?: (hex: string) => void;
}) {
  const [value, setValue] = useState(initial);
  return (
    <ColorField
      value={value}
      onChange={(hex) => {
        setValue(hex);
        onChange?.(hex);
      }}
    />
  );
}

describe("colorUtils", () => {
  it("expands 3-digit hex to canonical 6-digit lowercase", () => {
    expect(normalizeHex("#abc")).toBe("#aabbcc");
    expect(normalizeHex("ABC")).toBe("#aabbcc");
  });

  it("normalizes 6-digit hex (case + missing hash)", () => {
    expect(normalizeHex("#155DFC")).toBe("#155dfc");
    expect(normalizeHex("155dfc")).toBe("#155dfc");
  });

  it("rejects invalid hex", () => {
    expect(normalizeHex("#xyz")).toBeNull();
    expect(normalizeHex("#1234")).toBeNull();
    expect(normalizeHex("nope")).toBeNull();
    expect(isValidHex("#12g")).toBe(false);
  });

  it("coerces invalid input to #000000 for the native picker", () => {
    expect(toColorInputValue("garbage")).toBe("#000000");
    expect(toColorInputValue("#abc")).toBe("#aabbcc");
  });
});

describe("ColorField", () => {
  it("fires onChange while typing a hex value", () => {
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const input = screen.getByLabelText("Color");
    fireEvent.change(input, { target: { value: "#155dfc" } });
    expect(onChange).toHaveBeenCalledWith("#155dfc");
  });

  it("normalizes a valid value on blur", () => {
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const input = screen.getByLabelText("Color");
    fireEvent.change(input, { target: { value: "#ABC" } });
    fireEvent.blur(input);
    expect(onChange).toHaveBeenLastCalledWith("#aabbcc");
  });

  it("marks invalid input and shows the error message", () => {
    render(<Harness initial="not-a-color" />);
    const input = screen.getByLabelText("Color");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(
      screen.getByText("Enter a valid hex color, e.g. #155dfc"),
    ).toBeInTheDocument();
  });

  it("does not flag empty input as invalid", () => {
    render(<Harness initial="" />);
    const input = screen.getByLabelText("Color");
    expect(input).toHaveAttribute("aria-invalid", "false");
  });

  it("selecting a preset commits its canonical hex", () => {
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Use Blue"));
    expect(onChange).toHaveBeenCalledWith("#3399cc");
  });

  it("opens the native picker when the swatch is clicked", () => {
    render(<Harness initial="#155dfc" />);
    const swatch = screen.getByLabelText("Open color picker");
    // jsdom has no real picker; assert the hidden color input exists + is wired.
    expect(swatch).toBeInTheDocument();
    const native = document.querySelector(
      'input[type="color"]',
    ) as HTMLInputElement;
    expect(native).toBeTruthy();
    expect(native.value).toBe("#155dfc");
  });

  it("respects disabled", () => {
    render(<ColorField value="#155dfc" onChange={() => {}} disabled />);
    expect(screen.getByLabelText("Color")).toBeDisabled();
    expect(screen.getByLabelText("Open color picker")).toBeDisabled();
    expect(screen.getByLabelText("Use Blue")).toBeDisabled();
  });
});
