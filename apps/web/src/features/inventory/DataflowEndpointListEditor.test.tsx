import { useState } from "react";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import DataflowEndpointListEditor, { DATAFLOW_SIDE_LIMIT } from "./DataflowEndpointListEditor";
import type { DataflowEndpoint } from "./dataflows-api";

const api = vi.hoisted(() => ({ resolveDataflowEndpoint: vi.fn(), listAddressGroups: vi.fn(), listAllResources: vi.fn() }));
vi.mock("./dataflows-api", async original => ({ ...(await original()), resolveDataflowEndpoint: api.resolveDataflowEndpoint }));
vi.mock("./dataflowCatalog-api", async original => ({ ...(await original()), listAddressGroups: api.listAddressGroups }));
vi.mock("./api", () => ({ listAllResources: api.listAllResources }));

const device = (object_id: string): DataflowEndpoint => ({ object_type: "dcim.device", object_id, address_mode: "default", ipaddresses: [] });
const emptyResolution = {
  addresses: [], default_ipaddresses: [], diagnostics: [], ipaddresses: [], members: [],
  endpoint: { object_type: "dcim.device", object_id: "dev-1", address_mode: "default", ipaddresses: [] },
};

function Harness({ initial, onChange, showErrors = false }: { initial: DataflowEndpoint[]; onChange: (value: DataflowEndpoint[]) => void; showErrors?: boolean }) {
  const [value, setValue] = useState(initial);
  return <DataflowEndpointListEditor label="Source" addLabel="Add source" value={value} showErrors={showErrors}
    onChange={next => { setValue(next); onChange(next); }} />;
}

function mount(initial: DataflowEndpoint[], onChange: (value: DataflowEndpoint[]) => void = vi.fn(), showErrors = false) {
  render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <Harness initial={initial} onChange={onChange} showErrors={showErrors} />
  </QueryClientProvider>);
  return onChange;
}

async function choose(name: string, option: string) {
  const trigger = screen.getByRole("combobox", { name });
  await waitFor(() => expect(trigger).toBeEnabled());
  fireEvent.click(trigger);
  fireEvent.click(await screen.findByRole("option", { name: option }));
}

describe("DataflowEndpointListEditor", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    api.listAllResources.mockImplementation((entity: string) => {
      if (entity === "Devices") return Promise.resolve([
        { id: "dev-1", name: "Web host", type: "device" },
        { id: "dev-2", name: "Database host", type: "device" },
        { id: "dev-3", name: "Edge host", type: "device" },
      ]);
      if (entity === "Tags") return Promise.resolve([{ id: "tag-1", name: "Core", type: "tag" }]);
      if (entity === "IP Ranges") return Promise.resolve([{ id: "range-1", name: "Pool A", type: "ip_range" }]);
      return Promise.resolve([]);
    });
    api.listAddressGroups.mockResolvedValue([{ id: "ag-1", name: "Web tier" }]);
    api.resolveDataflowEndpoint.mockResolvedValue(emptyResolution);
  });

  it("keeps a fresh empty side neutral, then shows an inline error when requested", () => {
    mount([]);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add source" }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows an inline error for an empty side and adds the first row", () => {
    const onChange = mount([], vi.fn(), true);
    expect(screen.getByRole("alert")).toHaveTextContent("Add at least one object.");
    fireEvent.click(screen.getByRole("button", { name: "Add source" }));
    expect(onChange).toHaveBeenLastCalledWith([{ object_type: "dcim.device", object_id: null, address_mode: "default", ipaddresses: [] }]);
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    expect(screen.getByRole("alert")).toHaveTextContent("Select both a source and a destination.");
    expect(screen.queryByText("Add at least one object.")).not.toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Source 1" })).toBeInTheDocument();
  });

  it("removes a middle row and preserves the order of the remaining selectors", () => {
    const onChange = mount([device("dev-1"), device("dev-2"), device("dev-3")]);
    expect(screen.getByRole("group", { name: "Source 2" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Remove object Source 2" }));
    expect(onChange).toHaveBeenLastCalledWith([device("dev-1"), device("dev-3")]);
    expect(screen.getByRole("group", { name: "Source 2" })).toBeInTheDocument();
  });

  it("stops adding at the 32-row side limit", () => {
    mount(Array.from({ length: DATAFLOW_SIDE_LIMIT }, (_, index) => device(`dev-${index}`)));
    expect(screen.getByRole("group", { name: "Source 32" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add source" })).not.toBeInTheDocument();
  });

  it("requires a tag scope and offers OOB only for device tags", async () => {
    mount([device("dev-1")], vi.fn(), true);
    await choose("Source 1 type", "Tag");
    await choose("Source 1", "Core");
    expect(screen.getByRole("alert")).toHaveTextContent("Choose the tag scope.");
    await choose("Tag scope", "Devices");
    fireEvent.click(screen.getByRole("combobox", { name: "Source 1 addresses" }));
    expect(await screen.findByRole("option", { name: "OOB address" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Selected addresses" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "Default addresses" }));
    await choose("Tag scope", "Virtual machines");
    fireEvent.click(screen.getByRole("combobox", { name: "Source 1 addresses" }));
    expect(await screen.findByRole("option", { name: "All object addresses" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "OOB address" })).not.toBeInTheDocument();
  });

  it("leads an IP range preview with its resolved CIDRs and no zero-host card", async () => {
    api.resolveDataflowEndpoint.mockResolvedValue({
      ...emptyResolution,
      endpoint: { object_type: "ipam.iprange", object_id: "range-1", address_mode: "default", ipaddresses: [], label: "Pool A" },
      addresses: [{ cidr: "10.0.0.0/31", vrf_id: null }],
    });
    mount([device("dev-1")]);
    await choose("Source 1 type", "IP range");
    await choose("Source 1", "Pool A");
    fireEvent.click(screen.getByText("Inventory membership preview"));
    expect(await screen.findByText("IP range: Pool A")).toBeInTheDocument();
    expect(screen.getByText("10.0.0.0/31 · Default VRF")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "No known host members for this endpoint." })).not.toBeInTheDocument();
  });

  it("selects an address group through the catalog and summarizes it", async () => {
    api.resolveDataflowEndpoint.mockResolvedValue({
      ...emptyResolution,
      endpoint: { object_type: "inventory.addressgroup", object_id: "ag-1", address_mode: "default", ipaddresses: [], label: "Web tier" },
      addresses: [{ cidr: "10.0.0.0/24", vrf_id: null }],
    });
    const onChange = mount([device("dev-1")]);
    await choose("Source 1 type", "Address group");
    await choose("Source 1 · Address groups", "Web tier");
    expect(onChange).toHaveBeenLastCalledWith([{ object_type: "inventory.addressgroup", object_id: "ag-1", address_mode: "default", ipaddresses: [] }]);
    fireEvent.click(screen.getByText("Inventory membership preview"));
    expect(await screen.findByText("Address group: Web tier")).toBeInTheDocument();
    expect(screen.getByText("10.0.0.0/24 · Default VRF")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "No known host members for this endpoint." })).not.toBeInTheDocument();
  });
});
