import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useRef, useState } from "react";
import DataflowOverview from "./DataflowOverview";
import DataflowDetail from "./DataflowDetail";
import { DataflowDiagnostics, DataflowMemberList } from "./DataflowEndpointMembers";
import en from "@/shared/i18n/locales/en";
import type { Dataflow, DataflowEndpointResolution } from "./dataflows-api";

const resolve = vi.hoisted(() => vi.fn());
vi.mock("./dataflows-api", () => ({ resolveDataflowEndpoint: resolve }));

const flow = {
  id: "flow", name: "Region A to tenant B", description: "Application access", status: "active", effective_status: "active", blocked_by_group_id: null,
  transport: "tcp", destination_ports: [443, 8443], source_ports: null, icmp_types: null, allow_cross_vrf: false,
  sources: [{ object_type: "dcim.region", object_id: "region", label: "Region A", address_mode: "default", ipaddresses: [] }],
  destinations: [{ object_type: "tenancy.tenant", object_id: "tenant", label: "Tenant B", address_mode: "default", ipaddresses: [] }],
  application_id: null, group_id: null, comments: "", owner_id: null, tenant_id: null, tags: [], inherited_tags: [], effective_tags: [], custom_field_data: {},
} satisfies Dataflow;
const withFlow = (change: Partial<Dataflow>): Dataflow => ({ ...flow, ...change });
const resolution = {
  endpoint: flow.sources[0]!, ipaddresses: [], default_ipaddresses: [], addresses: [{ cidr: "192.0.2.1/32", vrf_id: null }], diagnostics: [],
  members: Array.from({ length: 120 }, (_, index) => ({ host: { object_type: "dcim.device", object_id: `host-${index}`, label: `Member ${index}` }, addresses: index === 119 ? [] : [{ cidr: `192.0.2.${index}/32`, vrf_id: null }], diagnostics: index === 119 ? [{ code: "inventory_excluded", message: "PRIVATE SERVER MESSAGE" }] : [] })),
} satisfies DataflowEndpointResolution;

function overview(flows: Dataflow[] = [flow], loading = false, failed = false, view: "list" | "overview" | "topology" | "matrix" = "list") {
  function Overview() {
    const [selected, setSelected] = useState<string | null>(null);
    const anchor = useRef<HTMLInputElement>(null);
    return <><input ref={anchor} aria-label="Search flows" /><DataflowOverview anchor={anchor} flows={flows} loading={loading} failed={failed} onRetry={vi.fn()} view={view} selected={flows.find(item => item.id === selected)} onSelect={setSelected} /></>;
  }
  return renderInRouter(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><Overview /></QueryClientProvider>);
}
function fill(template: string, values: Record<string, string | number>) {
  return template.replace(/\{\{(\w+)\}\}/g, (_, key: string) => String(values[key]));
}
function section(heading: string) {
  const element = screen.getByRole("heading", { name: heading }).closest("section");
  if (!element) throw new Error(`Missing section for ${heading}`);
  return element as HTMLElement;
}
beforeEach(() => { vi.clearAllMocks(); resolve.mockResolvedValue(resolution); });
const renderInRouter = (ui: Parameters<typeof render>[0]) => render(
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><MemoryRouter>{ui}</MemoryRouter></QueryClientProvider>,
);

describe("directed inventory overview", () => {
  it("renders the overview as catalog sections instead of a graph", () => {
    overview(Array.from({ length: 81 }, (_, index) => ({ ...flow, id: String(index) })), false, false, "overview");
    expect(screen.getByRole("region", { name: en.dataflows.groupedTitle })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: en.dataflows.overviewRelationships })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: en.dataflows.overviewObjects })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: en.dataflows.topologyTitle })).not.toBeInTheDocument();
  });
  it("renders relationship totals, a visible denominator and reveals the remainder", () => {
    const pairs: [Dataflow["sources"][number]["object_type"], Dataflow["sources"][number]["object_type"]][] = [
      ["ipam.service", "ipam.ipaddress"],
      ["tenancy.tenantgroup", "dcim.sitegroup"],
      ["dcim.location", "ipam.prefix"],
      ["virtualization.cluster", "dcim.device"],
      ["dcim.devicerole", "virtualization.virtualmachine"],
      ["dcim.platform", "dcim.manufacturer"],
      ["dcim.devicetype", "dcim.rack"],
      ["tenancy.tenant", "dcim.site"],
      ["virtualization.clustergroup", "virtualization.clustertype"],
      ["dcim.region", "tenancy.tenant"],
    ];
    const flows = pairs.flatMap(([sourceType, destinationType], index) => Array.from({ length: pairs.length - index }, (_, offset) => withFlow({
      id: `relationship-${index}-${offset}`,
      name: `Relationship ${index}-${offset}`,
      sources: [{ ...flow.sources[0]!, object_type: sourceType, object_id: `source-${index}`, label: `Source ${index}` }],
      destinations: [{ ...flow.destinations[0]!, object_type: destinationType, object_id: `destination-${index}`, label: `Destination ${index}` }],
    })));
    overview(flows, false, false, "overview");
    expect(screen.getByText(fill(en.dataflows.overviewRelationshipsShown, { shown: 8, total: 10 }))).toBeInTheDocument();
    const relationships = section(en.dataflows.overviewRelationships);
    expect(within(relationships).getAllByRole("button")).toHaveLength(9);
    const first = within(relationships).getByRole("button", { name: fill(en.dataflows.edgeBucket, { source: en.dataflows.endpointTypes.ipam_service, destination: en.dataflows.endpointTypes.ipam_ipaddress, count: 10 }) });
    expect(within(first).getByText(fill(en.dataflows.flowCount, { count: 10 }))).toBeInTheDocument();
    const remaining = within(relationships).queryByRole("button", { name: fill(en.dataflows.edgeBucket, { source: en.dataflows.endpointTypes.dcim_region, destination: en.dataflows.endpointTypes.tenancy_tenant, count: 1 }) });
    expect(remaining).not.toBeInTheDocument();
    fireEvent.click(within(relationships).getByRole("button", { name: en.dataflows.showAll }));
    expect(within(relationships).getByRole("button", { name: fill(en.dataflows.edgeBucket, { source: en.dataflows.endpointTypes.dcim_region, destination: en.dataflows.endpointTypes.tenancy_tenant, count: 1 }) })).toBeInTheDocument();
    expect(screen.getByText(fill(en.dataflows.overviewRelationshipsShown, { shown: 10, total: 10 }))).toBeInTheDocument();
  });
  it("opens exactly the relationship's original flows and inspects a selected flow", () => {
    const onSelect = vi.fn();
    const region = { ...flow.sources[0]!, object_type: "dcim.region" as const, object_id: "region", label: "Region A" };
    const tenant = { ...flow.destinations[0]!, object_type: "tenancy.tenant" as const, object_id: "tenant", label: "Tenant B" };
    const site = { ...flow.sources[0]!, object_type: "dcim.site" as const, object_id: "site", label: "Site C" };
    const cluster = { ...flow.destinations[0]!, object_type: "virtualization.cluster" as const, object_id: "cluster", label: "Cluster D" };
    renderInRouter(<DataflowOverview anchor={{ current: null }} flows={[
      withFlow({ id: "keep-1", name: "Keep one", sources: [region], destinations: [tenant] }),
      withFlow({ id: "keep-2", name: "Keep two", sources: [region], destinations: [tenant] }),
      withFlow({ id: "other", name: "Other flow", sources: [site], destinations: [cluster] }),
    ]} loading={false} failed={false} onRetry={vi.fn()} view="overview" onSelect={onSelect} />);
    fireEvent.click(screen.getByRole("button", { name: fill(en.dataflows.edgeBucket, { source: en.dataflows.endpointTypes.dcim_region, destination: en.dataflows.endpointTypes.tenancy_tenant, count: 2 }) }));
    const drilldown = screen.getByRole("region", { name: en.dataflows.bucketTitle });
    expect(within(drilldown).getByText(fill(en.dataflows.bucketCount, { count: 2, total: 2 }))).toBeInTheDocument();
    expect(within(drilldown).getByRole("button", { name: fill(en.dataflows.inspectFlow, { name: "Keep one" }) })).toBeInTheDocument();
    expect(within(drilldown).queryByRole("button", { name: fill(en.dataflows.inspectFlow, { name: "Other flow" }) })).not.toBeInTheDocument();
    fireEvent.click(within(drilldown).getByRole("button", { name: fill(en.dataflows.inspectFlow, { name: "Keep one" }) }));
    expect(onSelect).toHaveBeenCalledWith("keep-1");
  });
  it("searches every object and opens all incident flows beyond the ranking page", () => {
    const hub = (index: number) => ({ ...flow.sources[0]!, object_type: "dcim.device" as const, object_id: `hub-${index}`, label: `Hub ${index}` });
    const hubFlows = Array.from({ length: 7 }, (_, index) => hub(index)).flatMap((endpoint, index) => Array.from({ length: 3 }, (_, offset) => withFlow({
      id: `hub-${index}-${offset}`, name: `Hub flow ${index}-${offset}`, sources: [endpoint], destinations: [endpoint],
    })));
    const target = { ...flow.sources[0]!, object_type: "dcim.device" as const, object_id: "target-z", label: "Target Z" };
    const neighborA = { ...flow.sources[0]!, object_type: "virtualization.virtualmachine" as const, object_id: "neighbor-a", label: "Neighbor A" };
    const neighborB = { ...flow.sources[0]!, object_type: "virtualization.virtualmachine" as const, object_id: "neighbor-b", label: "Neighbor B" };
    renderInRouter(<DataflowOverview anchor={{ current: null }} flows={[
      ...hubFlows,
      withFlow({ id: "target-a", name: "Target to A", sources: [target], destinations: [neighborA] }),
      withFlow({ id: "b-target", name: "B to Target", sources: [neighborB], destinations: [target] }),
    ]} loading={false} failed={false} onRetry={vi.fn()} view="overview" onSelect={vi.fn()} />);
    expect(screen.getByText(fill(en.dataflows.overviewObjectsShown, { shown: 6, total: 10 }))).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: fill(en.dataflows.inspectObject, { name: "Target Z" }) })).not.toBeInTheDocument();
    const search = screen.getByRole("textbox", { name: en.dataflows.searchObjects });
    fireEvent.change(search, { target: { value: "Neighbor A" } });
    expect(screen.getByRole("button", { name: fill(en.dataflows.inspectObject, { name: "Neighbor A" }) })).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "target-z" } });
    expect(screen.getByText(fill(en.dataflows.overviewObjectsShown, { shown: 1, total: 10 }))).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: fill(en.dataflows.inspectObject, { name: "Neighbor A" }) })).not.toBeInTheDocument();
    const targetButton = screen.getByRole("button", { name: fill(en.dataflows.inspectObject, { name: "Target Z" }) });
    expect(within(targetButton).getByText(en.dataflows.endpointTypes.dcim_device)).toBeInTheDocument();
    expect(within(targetButton).getByText(fill(en.dataflows.objectFlowCount, { count: 2 }))).toBeInTheDocument();
    expect(within(targetButton).getByText(fill(en.dataflows.objectIncomingCount, { count: 1 }))).toBeInTheDocument();
    expect(within(targetButton).getByText(fill(en.dataflows.objectOutgoingCount, { count: 1 }))).toBeInTheDocument();
    fireEvent.click(targetButton);
    const drilldown = screen.getByRole("region", { name: en.dataflows.bucketTitle });
    expect(within(drilldown).getByText(fill(en.dataflows.bucketCount, { count: 2, total: 2 }))).toBeInTheDocument();
    expect(within(drilldown).getByRole("button", { name: fill(en.dataflows.inspectFlow, { name: "Target to A" }) })).toBeInTheDocument();
    expect(within(drilldown).getByRole("button", { name: fill(en.dataflows.inspectFlow, { name: "B to Target" }) })).toBeInTheDocument();
  });
  it("keeps the complete port selection available in the canonical detail panel", async () => {
    const ports = Array.from({ length: 256 }, (_, index) => 10000 + index);
    overview([{ ...flow, destination_ports: ports }]);
    fireEvent.click(screen.getByRole("button", { name: "Inspect Region A to tenant B" }));
    const dialog = await screen.findByRole("dialog", { name: flow.name });
    expect(within(dialog).getByText(ports.join(", "))).toBeInTheDocument();
  });
  it("keeps collection nodes compact and resolves only a selected connection", async () => {
    overview();
    expect(resolve).not.toHaveBeenCalled();
    const connection = screen.getByRole("button", { name: "Inspect Region A to tenant B" });
    expect(connection).toHaveAttribute("type", "button");
    connection.focus();
    expect(connection).toHaveFocus();
    expect(within(connection).getByText("Source · Region")).toBeInTheDocument();
    expect(within(connection).getByText("Destination · Tenant")).toBeInTheDocument();
    expect(within(connection).getByText("TCP")).toBeInTheDocument();
    expect(within(connection).getByText("443, 8443")).toBeInTheDocument();
    expect(within(connection).getByText("Active")).toBeInTheDocument();
    fireEvent.click(connection);
    expect(await screen.findByRole("dialog", { name: flow.name })).toBeInTheDocument();
    await waitFor(() => expect(resolve).toHaveBeenCalledTimes(2));
    expect(resolve).toHaveBeenCalledWith({ object_type: "dcim.region", object_id: "region", address_mode: "default", ipaddresses: [] });
    expect(resolve).toHaveBeenCalledWith({ object_type: "tenancy.tenant", object_id: "tenant", address_mode: "default", ipaddresses: [] });
    await waitFor(() => expect(screen.getAllByText("Member 119")).toHaveLength(2));
  });
  it("shows every known member including no-IP hosts and searches the complete list", () => {
    renderInRouter(<DataflowMemberList data={resolution} label="Source" />);
    expect(screen.getByRole("list", { name: "Source member hosts" }).children).toHaveLength(120);
    const hostList = screen.getByRole("list", { name: "Source member hosts" });
    expect(hostList).toHaveAttribute("tabindex", "0");
    expect(hostList).toHaveClass("overflow-y-auto");
    hostList.focus();
    expect(hostList).toHaveFocus();
    expect(screen.getByText("120 member hosts")).toBeInTheDocument();
    expect(screen.getByText("No chosen eligible IP addresses for this member.")).toBeInTheDocument();
    expect(screen.queryByText("PRIVATE SERVER MESSAGE")).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Search Source hosts or IPs…" }), { target: { value: "Member 119" } });
    expect(screen.getByRole("status")).toHaveTextContent("Showing 1 of 120 known hosts");
    expect(screen.getByText("Member 119")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Search Source hosts or IPs…" }), { target: { value: "192.0.2.118" } });
    expect(screen.getByText("Member 118")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: "Search Source hosts or IPs…" }), { target: { value: "missing" } });
    expect(screen.getByText("No hosts match this search.")).toBeInTheDocument();
  });
  it("does not print unknown diagnostic messages", () => {
    renderInRouter(<DataflowMemberList data={{ ...resolution, members: [], diagnostics: [{ code: "future_code", message: "INTERNAL DETAIL" }] }} label="Source" />);
    expect(screen.getByText(/This interaction could not be resolved safely/)).toBeInTheDocument();
    expect(screen.queryByText("INTERNAL DETAIL")).not.toBeInTheDocument();
  });
  it("deduplicates repeated diagnostics while preserving distinct server evidence", () => {
    const diagnostic = { code: "inventory_excluded", message: "same evidence" };
    renderInRouter(<DataflowMemberList data={{ ...resolution,
      diagnostics: [diagnostic, diagnostic, { ...diagnostic, message: "other evidence" }],
      members: [{ host: { object_type: "dcim.device", object_id: "host-0", label: "Member 0" }, addresses: [], diagnostics: [diagnostic, diagnostic] }],
    }} label="Source" />);
    expect(screen.getByRole("list", { name: "Resolution diagnostics" }).children).toHaveLength(2);
    const member = screen.getByRole("list", { name: "Source member hosts" }).children[0];
    expect(within(member as HTMLElement).getAllByText(/excluded/i)).toHaveLength(1);
  });
  it("keeps distinct policy provenance when deduplicating diagnostics", () => {
    const diagnostic = { code: "inventory_excluded", message: "same evidence", dataflow_id: "flow-1" };
    renderInRouter(<DataflowDiagnostics items={[diagnostic, diagnostic, { ...diagnostic, dataflow_id: "flow-2" }]} policy />);
    expect(screen.getByRole("list", { name: "Resolution diagnostics" }).children).toHaveLength(2);
    expect(screen.getByText("Contributing dataflows: flow-1")).toBeInTheDocument();
    expect(screen.getByText("Contributing dataflows: flow-2")).toBeInTheDocument();
    expect(screen.queryByText("same evidence")).not.toBeInTheDocument();
  });
  it("distinguishes Internet wildcard from All inventory and does not invent hosts", async () => {
    resolve.mockResolvedValue({ ...resolution, endpoint: { ...flow.sources[0]!, object_type: "internet", object_id: null }, members: [], addresses: [{ cidr: "0.0.0.0/0", vrf_id: null }] } satisfies DataflowEndpointResolution);
    overview([withFlow({ sources: [{ ...flow.sources[0]!, object_type: "internet", object_id: null }], destinations: [{ ...flow.destinations[0]!, object_type: "all", object_id: null }] })]);
    const connection = screen.getByRole("button", { name: "Inspect Region A to tenant B" });
    expect(within(connection).getByText("Source · Internet / any IP")).toBeInTheDocument();
    expect(within(connection).getByText("Destination · All Piping hosts")).toBeInTheDocument();
    fireEvent.click(connection);
    await waitFor(() => expect(screen.getAllByText("IP wildcard, not an inventory group. No concrete member hosts.")).toHaveLength(2));
  });
  it("renders loading and safe failure states", async () => {
    const view = overview([], true);
    expect(screen.getByRole("status")).toHaveTextContent("Loading connections");
    view.unmount();
    resolve.mockRejectedValue(new Error("SQL INTERNAL DETAIL"));
    overview();
    fireEvent.click(screen.getByRole("button", { name: "Inspect Region A to tenant B" }));
    await waitFor(() => expect(screen.getAllByRole("alert")).toHaveLength(2));
    expect(screen.queryByText(/SQL INTERNAL/)).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Retry" })).toHaveLength(2);
  });
  it("renders the shared empty state for an empty list and a retryable failure state", () => {
    const view = overview([], false, false);
    expect(screen.getByRole("heading", { name: "No matching dataflows." })).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Directed connections" })).not.toBeInTheDocument();
    view.unmount();
    const retry = vi.fn();
    renderInRouter(<DataflowOverview anchor={{ current: null }} flows={[]} loading={false} failed onRetry={retry} view="list" onSelect={vi.fn()} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Could not load dataflows. Check your access and try again.");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(retry).toHaveBeenCalled();
  });
  it("renders the shared empty state for endpoints without member hosts", () => {
    renderInRouter(<DataflowMemberList data={{ ...resolution, members: [] }} label="Source" />);
    expect(screen.getByRole("heading", { name: "No known host members for this endpoint." })).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Source member hosts" })).not.toBeInTheDocument();
    renderInRouter(<DataflowMemberList data={{ ...resolution, members: [], endpoint: { ...flow.sources[0]!, object_type: "internet", object_id: null } }} label="Source" />);
    expect(screen.getByRole("heading", { name: "IP wildcard, not an inventory group. No concrete member hosts." })).toBeInTheDocument();
  });
  it("resets each child member pane scope when the inspected parent flow changes", async () => {
    function Harness() {
      const [current, setCurrent] = useState<Dataflow | undefined>(undefined);
      return <>
        <button type="button" onClick={() => setCurrent(withFlow({ id: "a" }))}>Open a</button>
        <button type="button" onClick={() => setCurrent(withFlow({ id: "b" }))}>Open b</button>
        <DataflowDetail flow={current} onClose={vi.fn()} onCloseAutoFocus={vi.fn()} writeFailed={false} />
      </>;
    }
    renderInRouter(<Harness />);
    fireEvent.click(screen.getByRole("button", { name: "Open a" }));
    const search = await screen.findByRole("textbox", { name: "Search Region A hosts or IPs…" });
    fireEvent.change(search, { target: { value: "Member 119" } });
    expect(within(screen.getByRole("region", { name: "Region A member hosts" })).getByRole("status")).toHaveTextContent("Showing 1 of 120 known hosts");
    fireEvent.click(screen.getByText("Open b"));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Search Region A hosts or IPs…" })).toHaveValue(""));
    expect(within(screen.getByRole("region", { name: "Region A member hosts" })).getByRole("status")).toHaveTextContent("Showing 120 of 120 known hosts");
  });
});
