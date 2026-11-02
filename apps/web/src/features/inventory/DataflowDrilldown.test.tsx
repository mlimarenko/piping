import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { changeAppLanguage } from "@/shared/i18n";
import type { Dataflow } from "./dataflows-api";
import DataflowDrilldown from "./DataflowDrilldown";

const base: Dataflow = {
  id: "flow-a", name: "Application connection", description: "", status: "disabled", transport: "tcp",
  destination_ports: [443], allow_cross_vrf: false,
  source: { object_type: "dcim.device", object_id: "a", label: "Application", address_mode: "default", ipaddresses: [] },
  destination: { object_type: "dcim.device", object_id: "b", label: "Database", address_mode: "default", ipaddresses: [] },
};

afterEach(async () => { await act(() => changeAppLanguage("en")); });

describe("localized dataflow drilldown search", () => {
  it("finds the displayed status as well as its API value without including other statuses", async () => {
    await act(() => changeAppLanguage("ru"));
    render(<DataflowDrilldown flows={[base, { ...base, id: "flow-b", name: "Other connection", status: "active" }]} onInspect={vi.fn()} />);
    const search = screen.getByRole("textbox");
    for (const query of ["Отключён", "disabled"]) {
      fireEvent.change(search, { target: { value: query } });
      expect(screen.getAllByRole("button", { name: /Посмотреть поток/ })).toHaveLength(1);
      expect(within(screen.getByRole("list")).getByText(base.name)).toBeInTheDocument();
      expect(screen.queryByText("Other connection")).not.toBeInTheDocument();
    }
  });
});
