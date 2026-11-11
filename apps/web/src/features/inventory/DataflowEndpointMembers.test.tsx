import { screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { describe, expect, it } from "vitest";
import "@/shared/i18n";
import { DataflowMemberList } from "./DataflowEndpointMembers";

type Resolution = Parameters<typeof DataflowMemberList>[0]["data"];

function endpoint(object_type: string, label: string): Resolution["endpoint"] {
  return { object_type, object_id: "selector", address_mode: "default", ipaddresses: [], label } as Resolution["endpoint"];
}

function resolution(overrides: Partial<Resolution> = {}): Resolution {
  return {
    addresses: [], default_ipaddresses: [], diagnostics: [],
    endpoint: endpoint("ipam.iprange", "Pool A"),
    ipaddresses: [], members: [],
    ...overrides,
  } as Resolution;
}

const member = {
  host: { object_type: "dcim.device", object_id: "dev-1", label: "Member 0" },
  addresses: [{ cidr: "10.0.0.10/32", vrf_id: null }],
  diagnostics: [],
} as Resolution["members"][number];

describe("DataflowMemberList range, prefix and address-group readers", () => {
  it("leads an IP range with its resolved CIDRs and diagnostics instead of empty host cards", () => {
    render(<DataflowMemberList label="Source" data={resolution({
      addresses: [{ cidr: "10.0.0.0/31", vrf_id: null }, { cidr: "10.0.0.2/31", vrf_id: null }],
      diagnostics: [{ code: "inventory_excluded", message: "private" }],
    })} />);
    expect(screen.getByText("IP range: Pool A")).toBeInTheDocument();
    expect(screen.getByText("Resolved CIDRs (2)")).toBeInTheDocument();
    expect(screen.getByText("10.0.0.0/31 · Default VRF")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Resolution diagnostics" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "No known host members for this endpoint." })).not.toBeInTheDocument();
  });

  it("shows a prefix summary without inventing members", () => {
    render(<DataflowMemberList label="Source" data={resolution({
      endpoint: endpoint("ipam.prefix", "Office"),
      addresses: [{ cidr: "192.0.2.0/24", vrf_id: "vrf-1" }],
    })} />);
    expect(screen.getByText("Prefix: Office")).toBeInTheDocument();
    expect(screen.getByText("192.0.2.0/24 · vrf-1")).toBeInTheDocument();
    expect(screen.queryByText("0 member hosts")).not.toBeInTheDocument();
  });

  it("shows address-group CIDRs before existing member rows", () => {
    const { container } = render(<DataflowMemberList label="Source" data={resolution({
      endpoint: endpoint("inventory.addressgroup", "Web tier"),
      addresses: [{ cidr: "10.0.0.0/24", vrf_id: null }],
      members: [member],
    })} />);
    const text = container.textContent ?? "";
    expect(text).toContain("Address group: Web tier");
    expect(text.indexOf("10.0.0.0/24")).toBeLessThan(text.indexOf("Member 0"));
    expect(screen.getByRole("list", { name: "Source member hosts" }).children).toHaveLength(1);
  });

  it("keeps the host member view and its empty state unchanged", () => {
    render(<DataflowMemberList label="Source" data={resolution({
      endpoint: endpoint("dcim.device", "Web host"),
    })} />);
    expect(screen.getByText("0 member hosts")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "No known host members for this endpoint." })).toBeInTheDocument();
  });
});
