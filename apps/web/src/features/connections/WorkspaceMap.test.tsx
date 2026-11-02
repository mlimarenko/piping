import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionTarget, Resource } from "@/shared/api/types";
import "@/shared/i18n";
import WorkspaceMap from "./WorkspaceMap";

const mocks = vi.hoisted(() => ({ sites: vi.fn() }));
vi.mock("@/features/inventory/api", () => ({ listAllResources: mocks.sites }));
vi.mock("@/features/inventory/components/map/InventoryMapView", () => ({ default: ({ points, selectedId, onSelect }: { points: { id: string; labelCount: number; lat: number; lng: number }[]; selectedId?: string; onSelect: (id: string) => void }) => <div>{points.map(point => <button key={point.id} aria-pressed={selectedId === point.id} data-latitude={point.lat} data-longitude={point.lng} onClick={() => onSelect(point.id)}>Site {point.id}: {point.labelCount}</button>)}</div> }));
const targets: ConnectionTarget[] = [
  { id: "device-1" as ConnectionTarget["id"], name: "Gateway", targetKind: "device", siteId: "hq" as ConnectionTarget["siteId"] },
  { id: "vm-1" as ConnectionTarget["id"], name: "Worker", targetKind: "virtual_machine", siteId: "hq" as ConnectionTarget["siteId"] },
  { id: "vm-2" as ConnectionTarget["id"], name: "Unassigned", targetKind: "virtual_machine" },
];
const sites: Resource[] = [{ id: "hq", name: "Headquarters", type: "site", latitude: "0", longitude: "0" }];
const clients: QueryClient[] = [];
function mount(selectedTarget: ConnectionTarget | null = null, input = targets) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  const onSelect = vi.fn();
  render(<QueryClientProvider client={client}><WorkspaceMap targets={input} selectedTarget={selectedTarget} onSelect={onSelect} /></QueryClientProvider>);
  return { onSelect, client };
}
beforeEach(() => mocks.sites.mockReset().mockResolvedValue(sites));
afterEach(() => { cleanup(); clients.splice(0).forEach(client => client.clear()); });

describe("WorkspaceMap", () => {
  it("groups only the supplied targets, and selects targets through the shared inspector callback", async () => {
    const { onSelect } = mount(targets[0]);
    expect(await screen.findByRole("button", { name: "2 targets · 1 sites" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Site hq: 2" }));
    expect(onSelect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /Worker/ }));
    expect(onSelect).toHaveBeenLastCalledWith(targets[1]);
    expect(mocks.sites).toHaveBeenCalledWith("Sites");
  });

  it("refetches retained locations after the Inventory editor invalidates Sites", async () => {
    mocks.sites.mockResolvedValueOnce([{ ...sites[0], latitude: undefined }]);
    const { client } = mount();
    expect(await screen.findByText("No locations to plot")).toBeInTheDocument();
    await act(async () => {
      await client.invalidateQueries({ queryKey: ["inventory", "resources", "Sites"] });
    });
    expect(await screen.findByRole("button", { name: "2 targets · 1 sites" })).toBeInTheDocument();
    expect(mocks.sites).toHaveBeenCalledTimes(2);
  });

  it("plots and selects Frankfurt at Frankfurt rather than an Amsterdam regional anchor", async () => {
    mocks.sites.mockResolvedValue([
      { ...sites[0], latitude: "52.37", longitude: "4.9" },
      { id: "fra", name: "Frankfurt", type: "site", latitude: "50.11", longitude: "8.68" },
    ]);
    const frankfurt: ConnectionTarget = { ...targets[1], id: "fra-vm" as ConnectionTarget["id"], name: "Frankfurt worker", targetKind: "virtual_machine", siteId: "fra" as ConnectionTarget["siteId"] };
    const { onSelect } = mount(frankfurt, [...targets.slice(0, 1), frankfurt]);
    const marker = await screen.findByRole("button", { name: "Site fra: 1" });
    expect(marker).toHaveAttribute("data-latitude", "50.11");
    expect(marker).toHaveAttribute("data-longitude", "8.68");
    expect(marker).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Site hq: 1" })).toHaveAttribute("data-latitude", "52.37");
    fireEvent.click(marker);
    expect(onSelect).not.toHaveBeenCalled();
    fireEvent.click(within(screen.getByRole("list", { name: "Location targets" })).getByRole("button", { name: /Frankfurt worker/ }));
    expect(onSelect).toHaveBeenCalledWith(frankfurt);
  });

  it.each(["hover", "focus", "click"])("opens the unplaced list on %s and closes it with Escape", async method => {
    const { onSelect } = mount();
    const trigger = await screen.findByRole("button", { name: "Unplaced targets (1)" });
    if (method === "hover") fireEvent.pointerEnter(trigger, { pointerType: "mouse" });
    if (method === "focus") fireEvent.focus(trigger);
    if (method === "click") fireEvent.click(trigger);
    expect(await screen.findByText("No site assigned")).toBeInTheDocument();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await waitFor(() => expect(screen.queryByText("No site assigned")).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
    fireEvent.click(trigger);
    fireEvent.click(await screen.findByRole("button", { name: /Unassigned/ }));
    expect(onSelect).toHaveBeenCalledWith(targets[2]);
  });

  it("offers placed targets through an accessible list without relying on a canvas marker", async () => {
    const { onSelect } = mount();
    fireEvent.click(await screen.findByRole("button", { name: "2 targets · 1 sites" }));
    const locations = await screen.findByRole("complementary", { name: "Targets by site" });
    fireEvent.click(within(locations).getByRole("button", { name: /Headquarters/ }));
    expect(onSelect).not.toHaveBeenCalled();
    fireEvent.click(within(locations).getByRole("button", { name: /Worker/ }));
    expect(onSelect).toHaveBeenCalledWith(targets[1]);
  });

  it("bounds 500 mixed targets, searches canonical context, filters, pages and returns with Escape", async () => {
    const dense: ConnectionTarget[] = Array.from({ length: 500 }, (_, index) => ({ targetKind: index % 2 ? "virtual_machine" : "device", siteId: "hq" as ConnectionTarget["siteId"], id: `dense-${index}` as ConnectionTarget["id"], name: `Node ${String(index).padStart(3, "0")}`, primaryIp: `10.0.${Math.floor(index / 250)}.${index % 250}`, platform: index === 499 ? "Unique Platform" : "Linux" }));
    const { onSelect } = mount(null, dense);
    fireEvent.click(await screen.findByRole("button", { name: "Site hq: 500" }));
    expect(onSelect).not.toHaveBeenCalled();
    const list = screen.getByRole("list", { name: "Location targets" });
    expect(within(list).getAllByRole("button")).toHaveLength(25);
    expect(screen.getByText("500 of 500 targets")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(within(list).getByRole("button", { name: /Node 025/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "VMs 250" }));
    expect(screen.getByText("250 of 500 targets")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Search name, IP or context…" }), { target: { value: " unique PLATFORM " } });
    expect(screen.getByText("1 of 500 targets")).toBeInTheDocument();
    fireEvent.click(within(list).getByRole("button", { name: /Node 499/ }));
    expect(onSelect).toHaveBeenLastCalledWith(dense[499]);
    fireEvent.change(screen.getByRole("textbox", { name: "Search name, IP or context…" }), { target: { value: "not-found" } });
    expect(screen.getByText("No matching results")).toBeInTheDocument();
    const clearButton = screen.getAllByRole("button", { name: "Clear search and filters" }).at(0);
    if (!clearButton) throw new Error("Missing clear control");
    fireEvent.click(clearButton);
    expect(screen.getByText("500 of 500 targets")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Search name, IP or context…" }), { key: "Escape" });
    expect(screen.queryByRole("list", { name: "Location targets" })).toBeNull();
    await waitFor(() => expect(screen.getByRole("button", { name: "500 targets · 1 sites" })).toHaveFocus());
  });

  it("does not describe unresolved loading as missing coordinates", async () => {
    let resolve!: (value: Resource[]) => void;
    mocks.sites.mockReturnValue(new Promise<Resource[]>(done => { resolve = done; }));
    mount();
    expect(screen.getByRole("status")).toHaveTextContent("Loading site locations");
    expect(screen.queryByRole("button", { name: /Unplaced/ })).toBeNull();
    resolve(sites);
    expect(await screen.findByRole("button", { name: "2 targets · 1 sites" })).toBeInTheDocument();
  });

  it("does not publish stale placement counts after a site query fails, and retries", async () => {
    mocks.sites.mockRejectedValueOnce(new Error("private detail"));
    mount();
    expect(await screen.findByRole("alert")).toHaveTextContent("Missing-coordinate counts are unavailable");
    expect(screen.queryByText("private detail")).toBeNull();
    expect(screen.queryByRole("button", { name: /Unplaced/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("button", { name: "2 targets · 1 sites" })).toBeInTheDocument();
  });

  it("keeps unplaced targets inspectable when no locations can be plotted", async () => {
    mount(null, targets.slice(2));
    expect(await screen.findByText("No locations to plot")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Fit all locations" })).toBeDisabled();
    expect(screen.queryByText("Gateway")).toBeNull();
    expect(screen.getByRole("button", { name: "Unplaced targets (1)" })).toBeInTheDocument();
  });
});
