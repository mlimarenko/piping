import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import DetailedMap from "./DetailedMap";

const mocks = vi.hoisted(() => ({
  load: undefined as (() => void) | undefined,
  fitBounds: vi.fn(),
  setData: vi.fn(),
}));
vi.mock("maplibre-gl", () => ({
  addProtocol: vi.fn(),
  setWorkerUrl: vi.fn(),
  NavigationControl: class {},
  AttributionControl: class {},
  Map: class {
    addControl = vi.fn();
    addSource = vi.fn();
    addLayer = vi.fn();
    remove = vi.fn();
    fitBounds = mocks.fitBounds;
    getSource = () => ({ setData: mocks.setData });
    on(event: string, callback: unknown) {
      if (event === "load") mocks.load = callback as () => void;
    }
  },
}));

beforeEach(() => {
  mocks.load = undefined;
  mocks.fitBounds.mockReset();
  mocks.setData.mockReset();
  vi.stubGlobal("fetch", vi.fn((_url: string, options?: RequestInit) => Promise.resolve(options?.headers
    ? new Response(new TextEncoder().encode("PMTiles"), { status: 206 })
    : new Response(JSON.stringify({ version: 8, sources: { basemap: { type: "vector", url: "pmtiles:///maps/world.pmtiles" } }, layers: [] })))));
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
