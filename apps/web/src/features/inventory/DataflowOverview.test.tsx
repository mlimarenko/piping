import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useRef, useState } from "react";
import DataflowOverview from "./DataflowOverview";
import DataflowDetail from "./DataflowDetail";
import { DataflowDiagnostics, DataflowMemberList } from "./DataflowEndpointMembers";
import type { Dataflow, DataflowEndpointResolution } from "./dataflows-api";

const resolve = vi.hoisted(() => vi.fn());
vi.mock("./dataflows-api", () => ({ resolveDataflowEndpoint: resolve }));

const flow = {
  id: "flow", name: "Region A to tenant B", description: "Application access", status: "active", transport: "tcp", destination_ports: [443, 8443], allow_cross_vrf: false,
  source: { object_type: "dcim.region", object_id: "region", label: "Region A", address_mode: "default", ipaddresses: [] },
  destination: { object_type: "tenancy.tenant", object_id: "tenant", label: "Tenant B", address_mode: "default", ipaddresses: [] },
} satisfies Dataflow;
const resolution = {
  endpoint: flow.source, ipaddresses: [], default_ipaddresses: [], addresses: [{ cidr: "192.0.2.1/32", vrf_id: null }], diagnostics: [],
  members: Array.from({ length: 120 }, (_, index) => ({ host: { object_type: "dcim.device", object_id: `host-${index}`, label: `Member ${index}` }, addresses: index === 119 ? [] : [{ cidr: `192.0.2.${index}/32`, vrf_id: null }], diagnostics: index === 119 ? [{ code: "inventory_excluded", message: "PRIVATE SERVER MESSAGE" }] : [] })),
} satisfies DataflowEndpointResolution;

function overview(flows: Dataflow[] = [flow], loading = false, failed = false) {
  function Overview() {
    const [selected, setSelected] = useState<string | null>(null);
    const anchor = useRef<HTMLInputElement>(null);
    return <><input ref={anchor} aria-label="Search flows" /><DataflowOverview anchor={anchor} flows={flows} loading={loading} failed={failed} onRetry={vi.fn()} view="list" selected={flows.find(item => item.id === selected)} onSelect={setSelected} /></>;
  }
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><Overview /></QueryClientProvider>);
}
beforeEach(() => { vi.clearAllMocks(); resolve.mockResolvedValue(resolution); });

describe("directed inventory overview", () => {
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
    render(<DataflowMemberList data={resolution} label="Source" />);
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
    render(<DataflowMemberList data={{ ...resolution, members: [], diagnostics: [{ code: "future_code", message: "INTERNAL DETAIL" }] }} label="Source" />);
    expect(screen.getByText(/This interaction could not be resolved safely/)).toBeInTheDocument();
    expect(screen.queryByText("INTERNAL DETAIL")).not.toBeInTheDocument();
  });
  it("deduplicates repeated diagnostics while preserving distinct server evidence", () => {
    const diagnostic = { code: "inventory_excluded", message: "same evidence" };
    render(<DataflowMemberList data={{ ...resolution,
      diagnostics: [diagnostic, diagnostic, { ...diagnostic, message: "other evidence" }],
      members: [{ host: { object_type: "dcim.device", object_id: "host-0", label: "Member 0" }, addresses: [], diagnostics: [diagnostic, diagnostic] }],
    }} label="Source" />);
    expect(screen.getByRole("list", { name: "Resolution diagnostics" }).children).toHaveLength(2);
    const member = screen.getByRole("list", { name: "Source member hosts" }).children[0];
    expect(within(member as HTMLElement).getAllByText(/excluded/i)).toHaveLength(1);
  });
  it("keeps distinct policy provenance when deduplicating diagnostics", () => {
    const diagnostic = { code: "inventory_excluded", message: "same evidence", dataflow_id: "flow-1" };
    render(<DataflowDiagnostics items={[diagnostic, diagnostic, { ...diagnostic, dataflow_id: "flow-2" }]} policy />);
    expect(screen.getByRole("list", { name: "Resolution diagnostics" }).children).toHaveLength(2);
    expect(screen.getByText("Contributing dataflows: flow-1")).toBeInTheDocument();
    expect(screen.getByText("Contributing dataflows: flow-2")).toBeInTheDocument();
    expect(screen.queryByText("same evidence")).not.toBeInTheDocument();
  });
  it("distinguishes Internet wildcard from All inventory and does not invent hosts", async () => {
    resolve.mockResolvedValue({ ...resolution, endpoint: { ...flow.source, object_type: "internet", object_id: null }, members: [], addresses: [{ cidr: "0.0.0.0/0", vrf_id: null }] } satisfies DataflowEndpointResolution);
    overview([{ ...flow, source: { ...flow.source, object_type: "internet", object_id: null }, destination: { ...flow.destination, object_type: "all", object_id: null } }]);
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
    render(<DataflowOverview anchor={{ current: null }} flows={[]} loading={false} failed onRetry={retry} view="list" onSelect={vi.fn()} />);
    expect(screen.getByRole("alert")).toHaveTextContent("Could not load dataflows. Check your access and try again.");
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(retry).toHaveBeenCalled();
  });
  it("renders the shared empty state for endpoints without member hosts", () => {
    render(<DataflowMemberList data={{ ...resolution, members: [] }} label="Source" />);
    expect(screen.getByRole("heading", { name: "No known host members for this endpoint." })).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Source member hosts" })).not.toBeInTheDocument();
    render(<DataflowMemberList data={{ ...resolution, members: [], endpoint: { ...flow.source, object_type: "internet", object_id: null } }} label="Source" />);
    expect(screen.getByRole("heading", { name: "IP wildcard, not an inventory group. No concrete member hosts." })).toBeInTheDocument();
  });
  it("resets each child member pane scope when the inspected parent flow changes", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const detail = (id: string) => <QueryClientProvider client={client}><DataflowDetail flow={{ ...flow, id }} onClose={vi.fn()} onCloseAutoFocus={vi.fn()} writeFailed={false} /></QueryClientProvider>;
    const view = render(detail("a"));
    const search = await screen.findByRole("textbox", { name: "Search Source hosts or IPs…" });
    fireEvent.change(search, { target: { value: "Member 119" } });
    expect(within(screen.getByRole("region", { name: "Source member hosts" })).getByRole("status")).toHaveTextContent("Showing 1 of 120 known hosts");
    view.rerender(detail("b"));
    await waitFor(() => expect(screen.getByRole("textbox", { name: "Search Source hosts or IPs…" })).toHaveValue(""));
    expect(within(screen.getByRole("region", { name: "Source member hosts" })).getByRole("status")).toHaveTextContent("Showing 120 of 120 known hosts");
  });
});
