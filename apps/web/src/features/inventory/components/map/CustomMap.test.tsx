import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import CustomMap from "./CustomMap";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("CustomMap overview labels", () => {
  it.each([
    { color: undefined, fill: "hsl(var(--primary-fill))", foreground: "hsl(var(--primary-foreground))" },
    { color: "hsl(var(--status-info))", fill: "hsl(var(--status-info))", foreground: "hsl(var(--card))" },
  ])("pairs the rendered marker fill $fill with its count foreground", ({ color, fill, foreground }) => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    render(<CustomMap points={[{ id: "dense", lat: 0, lng: 0, label: "Dense location", labelCount: 505, color }]} onSelect={vi.fn()} />);
    const marker = screen.getByRole("button", { name: "Dense location" });
    const pin = marker.querySelector("circle[stroke-width]");
    expect(pin).toHaveAttribute("fill", fill);
    expect(pin).toHaveAttribute("stroke", fill);
    expect(screen.getByText("505")).toHaveAttribute("fill", foreground);
  });

  it("shows explicit dense counts and suppresses colliding labels without removing marker access", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    const points = [
      { id: "a", lat: 0, lng: 0, label: "Alpha location", radius: 14, labelCount: 500 },
      { id: "b", lat: 0, lng: 0.01, label: "Beta location", radius: 14, labelCount: 3 },
      { id: "c", lat: 0, lng: 0.02, label: "Gamma location", radius: 14, labelCount: 2 },
      { id: "d", lat: 60, lng: 100, label: "Distant location" },
    ];
    const onSelect = vi.fn();
    const { rerender } = render(<CustomMap points={points} onSelect={onSelect} />);
    expect(screen.getByText("500")).toBeInTheDocument();
    expect(screen.getAllByRole("button").at(-1)).toHaveAccessibleName("Alpha location");
    expect(screen.queryByText("99+")).toBeNull();
    expect(screen.getByText("Alpha location")).toBeInTheDocument();
    expect(screen.queryByText("Beta location")).toBeNull();
    expect(screen.queryByText("Gamma location")).toBeNull();
    const beta = screen.getByRole("button", { name: "Beta location" });
    fireEvent.keyDown(beta, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith("b");
    rerender(<CustomMap points={points} selectedId="b" onSelect={onSelect} />);
    expect(screen.getAllByRole("button").at(-1)).toHaveAccessibleName("Beta location");
    expect(screen.getByText("Beta location")).toBeInTheDocument();
    expect(screen.queryByText("Alpha location")).toBeNull();
    expect(screen.getByText("Distant location")).toBeInTheDocument();
  });

  it("places a location label above its pin when the label would cross a distinct Site badge", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    render(<CustomMap points={[
      { id: "a", lat: 0, lng: 0, label: "Alpha location", radius: 14, labelCount: 500 },
      { id: "b", lat: -8, lng: 3, label: "Beta location", radius: 14, labelCount: 4 },
      { id: "c", lat: 60, lng: 100, label: "Distant location" },
    ]} onSelect={vi.fn()} />);
    expect(Number(screen.getByText("Alpha location").getAttribute("y"))).toBeLessThan(0);
    expect(screen.getByRole("button", { name: "Beta location" })).toBeInTheDocument();
    expect(screen.getByText("4")).toBeInTheDocument();
  });
});
