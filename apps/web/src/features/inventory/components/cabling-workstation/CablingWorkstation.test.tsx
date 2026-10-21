import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { MemoryRouter, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as InventoryApi from "@/features/inventory/api";
import "@/shared/i18n";
import CablingWorkstation from "./CablingWorkstation";
import PageChrome from "@/shared/ui-shared/PageChrome";

const apiMocks = vi.hoisted(() => ({
  listCablingEndpoints: vi.fn(),
  traceCablingEndpoint: vi.fn(),
}));

vi.mock("@/features/inventory/api", async (importOriginal) => {
  const actual = await importOriginal<typeof InventoryApi>();
  return {
    ...actual,
    listCablingEndpoints: apiMocks.listCablingEndpoints,
    traceCablingEndpoint: apiMocks.traceCablingEndpoint,
  };
});

function renderWithClient(ui: ReactElement) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  let currentLocation = "";
  const LocationProbe = () => {
    const location = useLocation();
    currentLocation = `${location.pathname}${location.search}`;
    return null;
  };
  const result = render(
    <MemoryRouter>
      <LocationProbe />
      <QueryClientProvider client={client}>{ui}</QueryClientProvider>
    </MemoryRouter>,
  );
  return {
    ...result,
    getLocation: () => currentLocation,
  };
}

const endpoint = {
  endpoint: { kind: "power-outlet" as const, id: "outlet-1", label: "pdu-a · outlet 08" },
  parent: { entity: "Devices", id: "pdu-1", label: "pdu-a" },
  location: {
    site: { entity: "Sites", id: "site-1", label: "Amsterdam Edge" },
    rack: { entity: "Racks", id: "rack-1", label: "AMS-R1" },
    location: null,
  },
  link_state: "available" as const,
  trace_state: "unsupported" as const,
  cable: null,
  related_links: [{ entity: "Devices", id: "pdu-1", label: "pdu-a" }],
};

describe("CablingWorkstation", () => {
  beforeEach(() => {
    apiMocks.listCablingEndpoints.mockReset();
    apiMocks.traceCablingEndpoint.mockReset();
    apiMocks.listCablingEndpoints.mockResolvedValue({
      items: [endpoint],
      total: 1,
      limit: 50,
      offset: 0,
    });
    apiMocks.traceCablingEndpoint.mockResolvedValue({
      start: endpoint.endpoint,
      state: "unsupported",
      summary: { hop_count: 0, cable_count: 0, issue_count: 1 },
      segments: [],
      issues: [
        {
          code: "unsupported-endpoint",
          endpoint: endpoint.endpoint,
          message: "Endpoint kind is indexed but not traversable.",
        },
      ],
      related_links: endpoint.related_links,
      generated_at: "2026-07-06T00:00:00Z",
    });
  });

  it("shows the explicit start state before a query or filter", () => {
    renderWithClient(<><PageChrome title="Cabling Workstation" /><CablingWorkstation /></>);
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1, name: "Cabling Workstation" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 2, name: "Cabling Workstation" })).not.toBeInTheDocument();
    expect(screen.getByText("Search cabling endpoints")).toBeInTheDocument();
    expect(apiMocks.listCablingEndpoints).not.toHaveBeenCalled();
  });

  it("rejects short unfiltered searches in the UI", () => {
    renderWithClient(<CablingWorkstation />);
    fireEvent.change(screen.getByPlaceholderText("Search device, endpoint, cable, circuit..."), {
      target: { value: "p" },
    });
    expect(screen.getByText("Search is too short")).toBeInTheDocument();
    expect(apiMocks.listCablingEndpoints).not.toHaveBeenCalled();
  });

  it("renders unsupported trace state and issue after selecting a result", async () => {
    renderWithClient(<CablingWorkstation />);
    fireEvent.change(screen.getByPlaceholderText("Search device, endpoint, cable, circuit..."), {
      target: { value: "pdu" },
    });

    expect(await screen.findAllByText("pdu-a · outlet 08")).not.toHaveLength(0);
    await waitFor(() => expect(apiMocks.traceCablingEndpoint).toHaveBeenCalled());
    expect(await screen.findAllByText("Unsupported")).not.toHaveLength(0);
    expect(screen.getByText("Unsupported endpoint")).toBeInTheDocument();
  });

  it("navigates open actions through the router", async () => {
    const view = renderWithClient(<CablingWorkstation />);

    fireEvent.click(screen.getByRole("button", { name: /Open cable/i }));
    expect(view.getLocation()).toBe("/inventory/resources?entity=Cables");

    fireEvent.change(screen.getByPlaceholderText("Search device, endpoint, cable, circuit..."), {
      target: { value: "pdu" },
    });

    await waitFor(() => expect(apiMocks.traceCablingEndpoint).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "pdu-a" }));
    expect(view.getLocation()).toBe("/inventory/resources/Devices/pdu-1/edit");
  });

  it("exposes find/trace as real tabpanels and the mobile inspector as a dialog action outside the tablist", async () => {
    renderWithClient(<CablingWorkstation />);

    const panelOf = (name: string) => {
      const trigger = screen.getByRole("tab", { name });
      const controls = trigger.getAttribute("aria-controls");
      expect(controls).toBeTruthy();
      const panel = document.getElementById(controls as string);
      expect(panel).not.toBeNull();
      expect(panel).toHaveAttribute("role", "tabpanel");
    };

    // Only the two real panes are tabs; the inspector is never exposed as one.
    expect(screen.getAllByRole("tab")).toHaveLength(2);
    expect(screen.queryByRole("tab", { name: "Inspect" })).not.toBeInTheDocument();
    panelOf("Find");
    panelOf("Trace");

    fireEvent.keyDown(screen.getByRole("tab", { name: "Trace" }), { key: "Enter" });
    expect(screen.getByRole("tab", { name: "Trace" })).toHaveAttribute("aria-selected", "true");

    // The inspector action is a dialog trigger in the pane row, not a tab. Finder
    // rows also carry an "Inspect" button, so pick by the dialog contract.
    const [inspectButton] = screen.getAllByRole("button", { name: "Inspect" }).filter((button) => button.getAttribute("aria-haspopup") === "dialog");
    if (!inspectButton) throw new Error("mobile inspector dialog trigger missing");
    expect(inspectButton).toHaveAttribute("aria-expanded", "false");

    act(() => inspectButton.focus());
    expect(inspectButton).toHaveFocus();
    fireEvent.click(inspectButton);

    const dialog = await screen.findByRole("dialog", { name: "Inspector" });
    expect(dialog).toBeInTheDocument();

    // The Sheet renders an icon close (X) and the footer Close action; take the text one.
    const [closeAction] = within(dialog).getAllByRole("button", { name: "Close" }).filter((button) => !button.querySelector("svg"));
    if (!closeAction) throw new Error("sheet close action missing");
    fireEvent.click(closeAction);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    // Focus returns to the action, and cancelling keeps the find/trace selection
    // and the underlying pane instead of forcing the pane to trace.
    await waitFor(() => expect(inspectButton).toHaveFocus());
    expect(screen.getByRole("tab", { name: "Trace" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel", { name: "Trace" })).toBeInTheDocument();
  });
});
