import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { changeAppLanguage } from "@/shared/i18n";
import type { Dataflow } from "./dataflows-api";
import DataflowDrilldown from "./DataflowDrilldown";

const endpoint = (object_id: string, label: string) => ({ object_type: "dcim.device" as const, object_id, label, address_mode: "default" as const, ipaddresses: [] });

const base: Dataflow = {
  id: "flow-a", name: "Application connection", description: "", comments: "", status: "disabled", effective_status: "disabled",
  blocked_by_group_id: null, transport: "tcp", destination_ports: [443], source_ports: null, icmp_types: null,
  allow_cross_vrf: false, application_id: null, group_id: null, owner_id: null, tenant_id: null,
  tags: [], inherited_tags: [], effective_tags: [], custom_field_data: {},
  sources: [endpoint("a", "Application"), endpoint("a2", "Application two"), endpoint("a3", "Application three")],
  destinations: [endpoint("b", "Database")],
};

afterEach(async () => { await act(() => changeAppLanguage("en")); });

describe("localized dataflow drilldown search", () => {
  it("keeps the readable multi-endpoint direction without dumping raw type and identifier rows", () => {
    render(<DataflowDrilldown flows={[base]} onInspect={vi.fn()} />);
    expect(screen.getByText("Application +2 → Database")).toBeInTheDocument();
    expect(screen.queryByText(/dcim\.device · a/)).not.toBeInTheDocument();
  });
  it("finds the displayed effective status as well as its API value without including other statuses", async () => {
    await act(() => changeAppLanguage("ru"));
    render(<DataflowDrilldown flows={[base, { ...base, id: "flow-b", name: "Other connection", status: "active", effective_status: "active" }]} onInspect={vi.fn()} />);
    const search = screen.getByRole("textbox");
    for (const query of ["Отключён", "disabled"]) {
      fireEvent.change(search, { target: { value: query } });
      expect(screen.getAllByRole("button", { name: /Посмотреть поток/ })).toHaveLength(1);
      expect(screen.getByText(base.name)).toBeInTheDocument();
      expect(screen.queryByText("Other connection")).not.toBeInTheDocument();
    }
  });
  it("searches destination and source ports and ICMP types", () => {
    render(<DataflowDrilldown flows={[base, { ...base, id: "flow-icmp", name: "ICMP flow", transport: "icmp", destination_ports: null, icmp_types: [8] }]} onInspect={vi.fn()} />);
    const search = screen.getByRole("textbox");
    fireEvent.change(search, { target: { value: "443" } });
    expect(screen.getByText(base.name)).toBeInTheDocument();
    expect(screen.queryByText("ICMP flow")).not.toBeInTheDocument();
    fireEvent.change(search, { target: { value: "ICMP flow" } });
    expect(screen.getByText("ICMP flow")).toBeInTheDocument();
  });
});
