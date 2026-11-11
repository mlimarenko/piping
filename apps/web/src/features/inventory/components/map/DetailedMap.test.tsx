import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import DetailedMap from "./DetailedMap";
import { groupMapPoints } from "./mapGroups";

const mocks = vi.hoisted(() => ({
  load: undefined as (() => void) | undefined,
  fitBounds: vi.fn(),
  setData: vi.fn(),
  easeTo: vi.fn(),
  handlers: new Map<string, (event: unknown) => void>(),
  canvas: { style: { cursor: "" }, title: "" },
  zoom: 3,
  addLayer: vi.fn(),
  mapOptions: vi.fn(),
}));
vi.mock("maplibre-gl", () => ({
  addProtocol: vi.fn(),
  setWorkerUrl: vi.fn(),
  NavigationControl: class {},
  AttributionControl: class {},
  Map: class {
    constructor(options: unknown) { mocks.mapOptions(options); }
    addControl = vi.fn();
    addSource = vi.fn();
    addLayer = mocks.addLayer;
    remove = vi.fn();
    fitBounds = mocks.fitBounds;
    easeTo = mocks.easeTo;
    getZoom = () => mocks.zoom;
    getMaxZoom = () => 22;
    getCanvas = () => mocks.canvas;
    getSource = () => ({ setData: mocks.setData });
    on(event: string, layerOrCallback: unknown, callback?: unknown) {
      if (typeof layerOrCallback === "string" && typeof callback === "function") mocks.handlers.set(`${event}:${layerOrCallback}`, callback as (event: unknown) => void);
      if (typeof layerOrCallback === "function") mocks.handlers.set(event, layerOrCallback as (event: unknown) => void);
      if (event === "load") mocks.load = layerOrCallback as () => void;
    }
  },
}));

beforeEach(() => {
  mocks.load = undefined;
  mocks.fitBounds.mockReset();
  mocks.setData.mockReset();
  mocks.easeTo.mockReset();
  mocks.addLayer.mockReset();
  mocks.mapOptions.mockReset();
  mocks.handlers.clear();
  mocks.canvas.title = "";
  mocks.zoom = 3;
  vi.stubGlobal("fetch", vi.fn((_url: string, options?: RequestInit) => Promise.resolve(options?.headers
    ? new Response(new TextEncoder().encode("PMTiles"), { status: 206 })
    : new Response(JSON.stringify({ version: 8, sources: { basemap: { type: "vector", url: "pmtiles:///maps/world.pmtiles" } }, layers: [] })))));
});

it("passes a zero pitch range and disables pitch gestures to MapLibre", async () => {
  render(<DetailedMap points={[]} />);
  await waitFor(() => expect(mocks.mapOptions).toHaveBeenCalledOnce());
  expect(mocks.mapOptions).toHaveBeenCalledWith(expect.objectContaining({
    pitch: 0,
    minPitch: 0,
    maxPitch: 0,
    touchPitch: false,
    pitchWithRotate: false,
    renderWorldCopies: true,
  }));
});

it("exposes the exact detailed count on marker hover", async () => {
  render(<DetailedMap points={[{ id: "a", label: "Alpha", lat: 0, lng: 0, labelCount: 1234567 }]} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  act(() => mocks.load?.());
  act(() => mocks.handlers.get("mouseenter:piping-inventory-sites-circle")?.({ features: [{ properties: { label: "Alpha", count: 1234567 } }] }));
  expect(mocks.canvas.title).toBe("Alpha: 1234567");
  act(() => mocks.handlers.get("mouseleave:piping-inventory-sites-circle")?.({}));
  expect(mocks.canvas.title).toBe("");
});

it("renders readable centered count badges and a distinct cluster label and ring", async () => {
  render(<DetailedMap points={[{ id: "a", lat: 0, lng: 0, labelCount: 999999 }, { id: "b", lat: 0, lng: 0 }]} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  act(() => mocks.load?.());
  const layers = mocks.addLayer.mock.calls.map(([layer]) => layer as { id: string; layout?: Record<string, unknown> });
  const count = layers.find(layer => layer.id === "piping-inventory-sites-count");
  expect(count?.layout?.["text-size"]).toBeGreaterThanOrEqual(12);
  expect(count?.layout?.["text-offset"]).toEqual([0, 0]);
  expect(count?.layout?.["text-allow-overlap"]).toBe(true);
  expect(count?.layout?.["text-ignore-placement"]).toBe(true);
  expect(layers.find(layer => layer.id === "piping-inventory-cluster-ring")).toBeDefined();
  expect(layers.find(layer => layer.id === "piping-inventory-cluster-label")?.layout?.["text-field"]).toEqual(["get", "clusterLabel"]);
});

it("offers keyboard activation of a nearby group without selecting an arbitrary point", async () => {
  const onSelect = vi.fn();
  render(<DetailedMap points={[{ id: "a", lat: 0, lng: 0, labelCount: 500 }, { id: "b", lat: 0, lng: 0.1, labelCount: 7 }]} onSelect={onSelect} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  act(() => mocks.load?.());
  mocks.easeTo.mockClear();
  fireEvent.click(screen.getByRole("button", { name: "2 locations" }));
  expect(mocks.easeTo).toHaveBeenCalledWith({ center: [expect.closeTo(0.05), expect.closeTo(0)], zoom: 5, duration: 350 });
  expect(onSelect).not.toHaveBeenCalled();
  mocks.zoom = 16;
  act(() => mocks.handlers.get("zoomend")?.({}));
  expect(screen.getByRole("button", { name: "a" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "b" })).toBeInTheDocument();
});

it("reveals co-located members and selects only the chosen one", async () => {
  const onSelect = vi.fn();
  render(<DetailedMap points={[{ id: "a", label: "Alpha", lat: 0, lng: 0 }, { id: "b", label: "Beta", lat: 0, lng: 0 }]} onSelect={onSelect} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  act(() => mocks.load?.());
  const cluster = screen.getByRole("button", { name: "2 locations" });
  cluster.focus();
  fireEvent.click(cluster);
  expect(onSelect).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Alpha" })).toHaveFocus();
  fireEvent.keyDown(screen.getByRole("button", { name: "Alpha" }), { key: "Escape" });
  expect(cluster).toHaveFocus();
  fireEvent.click(cluster);
  fireEvent.click(screen.getByRole("button", { name: "Beta" }));
  expect(onSelect).toHaveBeenCalledWith("b");
});

it("zooms toward nearby dateline members without selecting one", async () => {
  mocks.zoom = 8;
  const onSelect = vi.fn();
  render(<DetailedMap points={[{ id: "east", lat: 0, lng: 179.99 }, { id: "west", lat: 0, lng: -179.99 }]} onSelect={onSelect} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  act(() => mocks.load?.());
  fireEvent.click(screen.getByRole("button", { name: "2 locations" }));
  expect(mocks.easeTo).toHaveBeenCalledOnce();
  const options = mocks.easeTo.mock.calls[0]![0] as { center: [number, number]; zoom: number; duration: number };
  expect(Math.abs(options.center[0])).toBeCloseTo(180, 5);
  expect(options.center[1]).toBeCloseTo(0);
  expect(options).toMatchObject({ zoom: 10, duration: 350 });
  expect(onSelect).not.toHaveBeenCalled();
});

it("reveals both seam-equivalent locations instead of zooming or selecting arbitrarily", async () => {
  const onSelect = vi.fn();
  render(<DetailedMap points={[{ id: "east", label: "East", lat: 0, lng: 180 }, { id: "west", label: "West", lat: 0, lng: -180 }]} onSelect={onSelect} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  act(() => mocks.load?.());
  mocks.easeTo.mockClear();
  fireEvent.click(screen.getByRole("button", { name: "2 locations" }));
  expect(mocks.easeTo).not.toHaveBeenCalled();
  expect(onSelect).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "East" })).toHaveFocus();
  fireEvent.click(screen.getByRole("button", { name: "West" }));
  expect(onSelect).toHaveBeenCalledWith("west");
});

it("uses the same group activation for a map click", async () => {
  const points = [{ id: "a", lat: 0, lng: 0 }, { id: "b", lat: 0, lng: 0 }];
  const onSelect = vi.fn();
  render(<DetailedMap points={points} onSelect={onSelect} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  act(() => mocks.load?.());
  const preventDefault = vi.fn();
  const id = groupMapPoints(points, 4096, "wrapped")[0]!.point.id;
  act(() => mocks.handlers.get("click:piping-inventory-sites-circle")?.({ features: [{ properties: { id } }], preventDefault }));
  expect(preventDefault).toHaveBeenCalledOnce();
  expect(onSelect).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "b" })).toBeInTheDocument();
});

it("keeps the camera on polling and selection, and moves only on explicit focus or fit", async () => {
  const points = [{ id: "a", lat: 0, lng: 0 }, { id: "b", lat: 20, lng: 30 }];
  const view = render(<DetailedMap points={points} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  act(() => mocks.load?.());
  expect(mocks.fitBounds).toHaveBeenCalledTimes(1);
  view.rerender(<DetailedMap points={points.map(point => ({ ...point }))} selectedId="b" />);
  expect(mocks.fitBounds).toHaveBeenCalledTimes(1);
  expect(mocks.easeTo).not.toHaveBeenCalled();
  view.rerender(<DetailedMap points={points} selectedId="b" focusId="b" focusKey={1} />);
  expect(mocks.easeTo).toHaveBeenCalledWith({ center: [30, 20], zoom: 8, duration: 350 });
  view.rerender(<DetailedMap points={points} focusId="b" focusKey={1} fitKey={1} />);
  expect(mocks.fitBounds).toHaveBeenCalledTimes(2);
});

it("fulfils a pending focus once when its point arrives", async () => {
  const view = render(<DetailedMap points={[{ id: "a", lat: 0, lng: 0 }]} focusId="b" />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  act(() => mocks.load?.());
  expect(mocks.easeTo).not.toHaveBeenCalled();
  view.rerender(<DetailedMap points={[{ id: "b", lat: 20, lng: 30 }]} focusId="b" />);
  expect(mocks.easeTo).toHaveBeenCalledTimes(1);
  view.rerender(<DetailedMap points={[{ id: "b", lat: 21, lng: 30 }]} focusId="b" />);
  expect(mocks.easeTo).toHaveBeenCalledTimes(1);
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it("frames stable Tokyo targets when asynchronous map initialization becomes ready", async () => {
  const points = [{ id: "tokyo", lat: 35.6762, lng: 139.6503 }];
  const onReady = vi.fn();
  render(<DetailedMap points={points} onReady={onReady} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  expect(mocks.fitBounds).not.toHaveBeenCalled();
  act(() => mocks.load?.());
  expect(mocks.fitBounds).toHaveBeenCalledWith(
    [[expect.closeTo(139.2503), expect.closeTo(35.2762)], [expect.closeTo(140.0503), expect.closeTo(36.0762)]],
    { padding: 72, duration: 0 },
  );
  expect(onReady).toHaveBeenCalledOnce();
});

it("frames the latest targets if the selection changes before map load", async () => {
  const view = render(<DetailedMap points={[{ id: "london", lat: 51.5, lng: -0.12 }]} />);
  await waitFor(() => expect(mocks.load).toBeDefined());
  view.rerender(<DetailedMap points={[{ id: "tokyo", lat: 35, lng: 139 }]} />);
  act(() => mocks.load?.());
  expect(mocks.fitBounds).toHaveBeenCalledWith([[138.6, 34.6], [139.4, 35.4]], { padding: 72, duration: 0 });
});
