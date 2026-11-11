import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import CustomMap from "./CustomMap";

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe("CustomMap overview labels", () => {
  it("keeps dateline endpoints separate on the nonwrapping SVG world", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    render(<CustomMap points={[{ id: "east", label: "East", lat: 0, lng: 180 }, { id: "west", label: "West", lat: 0, lng: -180 }]} onSelect={vi.fn()} />);
    expect(screen.getByRole("button", { name: "East" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "West" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "2 locations" })).toBeNull();
  });
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

  it("groups nearby locations and zooms rather than selecting a member", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    const points = [
      { id: "a", lat: 0, lng: 0, label: "Alpha location", radius: 14, labelCount: 500 },
      { id: "b", lat: 0, lng: 0.1, label: "Beta location", radius: 14, labelCount: 3 },
      { id: "c", lat: 0, lng: 0.4, label: "Gamma location", radius: 14, labelCount: 2 },
      { id: "d", lat: 60, lng: 100, label: "Distant location" },
    ];
    const onSelect = vi.fn();
    const view = render(<CustomMap points={points} onSelect={onSelect} />);
    const before = view.container.querySelector("svg > g")?.getAttribute("transform");
    const cluster = screen.getByRole("button", { name: "3 locations" });
    expect(cluster).toHaveTextContent("3");
    expect(cluster.querySelectorAll("text")[1]).toHaveTextContent("3 locations");
    fireEvent.keyDown(cluster, { key: "Enter" });
    expect(onSelect).not.toHaveBeenCalled();
    expect(view.container.querySelector("svg > g")?.getAttribute("transform")).not.toBe(before);
    expect(screen.getByRole("button", { name: "Distant location" })).toBeInTheDocument();
  });

  it("reveals every co-located member for keyboard selection", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    const onSelect = vi.fn();
    render(<CustomMap points={[{ id: "a", lat: 0, lng: 0, label: "Alpha" }, { id: "b", lat: 0, lng: 0, label: "Beta" }]} onSelect={onSelect} />);
    const cluster = screen.getByRole("button", { name: "2 locations" });
    cluster.focus();
    fireEvent.keyDown(cluster, { key: " " });
    expect(onSelect).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Alpha" })).toHaveFocus();
    expect(screen.getByRole("button", { name: "Alpha" })).toHaveClass("min-h-11");
    expect(screen.getByRole("button", { name: "Close" })).toHaveClass("h-11");
    fireEvent.keyDown(screen.getByRole("button", { name: "Alpha" }), { key: "Escape" });
    expect(cluster).toHaveFocus();
    fireEvent.keyDown(cluster, { key: "Enter" });
    fireEvent.click(screen.getByRole("button", { name: "Beta" }));
    expect(onSelect).toHaveBeenCalledWith("b");
  });

  it("keeps separate location markers and counts accessible", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    render(<CustomMap points={[
      { id: "a", lat: 0, lng: 0, label: "Alpha location", radius: 14, labelCount: 500 },
      { id: "b", lat: -18, lng: 3, label: "Beta location", radius: 14, labelCount: 4 },
      { id: "c", lat: 60, lng: 100, label: "Distant location" },
    ]} onSelect={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Alpha location" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Beta location" })).toBeInTheDocument();
    expect(screen.getByText("4")).toBeInTheDocument();
  });

  it("centers compact counts while retaining the exact value on hover", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    render(<CustomMap points={[{ id: "a", lat: 0, lng: 0, label: "Alpha", labelCount: 1234567 }]} onSelect={vi.fn()} />);
    expect(screen.getByText("1M")).toHaveAttribute("text-anchor", "middle");
    expect(screen.getByRole("button", { name: "Alpha" }).querySelector("title")).toHaveTextContent("1234567");
  });

  it("fits readable four-character counts inside a 44px hit target", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    render(<CustomMap points={[{ id: "a", lat: 0, lng: 0, label: "Alpha", labelCount: 999999 }]} onSelect={vi.fn()} />);
    const marker = screen.getByRole("button", { name: "Alpha" });
    expect(marker.querySelector('circle[fill="transparent"]')).toHaveAttribute("r", "22");
    expect(screen.getByText("999k")).toHaveAttribute("font-size", "12");
    expect(marker.querySelector("title")).toHaveTextContent("999999");
    expect(Number(marker.querySelector("circle[stroke-width]")?.getAttribute("r"))).toBeGreaterThanOrEqual(20);
  });

  it("preserves the focused geographic center and scale when resized", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    let notify: ResizeObserverCallback | undefined;
    const originalObserver = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class { constructor(callback: ResizeObserverCallback) { notify = callback; } observe() {} unobserve() {} disconnect() {} };
    const view = render(<CustomMap points={[{ id: "a", lat: 0, lng: 0 }, { id: "b", lat: 20, lng: 30 }]} focusId="b" />);
    const transform = () => view.container.querySelector("svg > g")?.getAttribute("transform") ?? "";
    const parse = () => transform().match(/translate\(([-\d.]+) ([-\d.]+)\) scale\(([-\d.]+)\)/)!.slice(1).map(Number);
    const [x, y, scale] = parse();
    expect(scale).toBeGreaterThanOrEqual(512 * 2 ** 8);
    act(() => notify?.([{ contentRect: { width: 600, height: 400 } } as ResizeObserverEntry], {} as ResizeObserver));
    const [nextX, nextY, nextScale] = parse();
    expect(nextScale).toBe(scale);
    expect(nextX).toBeCloseTo(x! - 100);
    expect(nextY).toBeCloseTo(y! - 50);
    globalThis.ResizeObserver = originalObserver;
  });

  it("does not move for polling or selection, but focuses on request", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    const points = [{ id: "a", lat: 0, lng: 0 }, { id: "b", lat: 20, lng: 30 }];
    const view = render(<CustomMap points={points} onSelect={vi.fn()} />);
    const transform = () => view.container.querySelector("svg > g")?.getAttribute("transform");
    const initial = transform();
    view.rerender(<CustomMap points={points.map(point => ({ ...point }))} selectedId="b" onSelect={vi.fn()} />);
    expect(transform()).toBe(initial);
    view.rerender(<CustomMap points={points} selectedId="b" focusId="b" focusKey={1} onSelect={vi.fn()} />);
    expect(transform()).not.toBe(initial);
  });

  it("fulfils a pending focus when its point arrives, without refocusing on polling", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new DOMRect(0, 0, 800, 500));
    const initial = [{ id: "a", lat: 0, lng: 0 }];
    const view = render(<CustomMap points={initial} focusId="b" />);
    const transform = () => view.container.querySelector("svg > g")?.getAttribute("transform");
    const before = transform();
    const points = [...initial, { id: "b", lat: 20, lng: 30 }];
    view.rerender(<CustomMap points={points} focusId="b" />);
    expect(transform()).not.toBe(before);
    const focused = transform();
    view.rerender(<CustomMap points={points.map(point => ({ ...point, lat: point.lat + 1 }))} focusId="b" />);
    expect(transform()).toBe(focused);
  });
});
