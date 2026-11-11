import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "i18next";
import DataflowTopology from "./DataflowTopology";
import DataflowMatrix from "./DataflowMatrix";
import { buildDataflowVisualization } from "./dataflowVisualization";
import en from "@/shared/i18n/locales/en";
import type { Dataflow } from "./dataflows-api";

const endpoint: Dataflow["sources"][number] = { object_type: "dcim.device", object_id: "host-1", label: "Shared host", address_mode: "selected", ipaddresses: ["ip-b", "ip-a"] };
const flow: Dataflow = {
  id: "a", name: "First", description: "", status: "active", effective_status: "active", blocked_by_group_id: null,
  transport: "tcp", destination_ports: [443], source_ports: null, icmp_types: null, allow_cross_vrf: false,
  sources: [{ ...endpoint, ipaddresses: [...endpoint.ipaddresses] }], destinations: [{ ...endpoint, object_id: "host-2", label: "Database", ipaddresses: [] }],
  application_id: null, group_id: null, comments: "", owner_id: null, tenant_id: null, tags: [], inherited_tags: [], effective_tags: [], custom_field_data: {},
};

const makeNode = (object_id: string, label: string): Dataflow["sources"][number] => ({ ...endpoint, object_id, label });
const makeFlow = (source: Dataflow["sources"][number], destination: Dataflow["sources"][number], id: string, name = id): Dataflow => ({ ...flow, id, name, sources: [source], destinations: [destination] });

let viewportWidth = 1609;
let viewportHeight = 560;
const scrollTo = vi.fn<(options?: ScrollToOptions) => void>();

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get(this: HTMLElement) { return this.getAttribute("aria-label") === en.dataflows.topologyCanvas ? viewportWidth : 0; },
  });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) { return this.getAttribute("aria-label") === en.dataflows.topologyCanvas ? viewportHeight : 0; },
  });
  scrollTo.mockImplementation(function (this: HTMLElement, options?: ScrollToOptions) {
    if (!options) return;
    this.scrollLeft = options.left ?? 0;
    this.scrollTop = options.top ?? 0;
  });
  Object.defineProperty(HTMLElement.prototype, "scrollTo", { configurable: true, writable: true, value: scrollTo });
});

beforeEach(() => {
  viewportWidth = 1609;
  viewportHeight = 560;
  scrollTo.mockClear();
});

function mockResizeObserver(width: number, height: number) {
  const original = window.ResizeObserver;
  class MockResizeObserver {
    constructor(private readonly callback: ResizeObserverCallback) {}
    observe(target: Element) {
      this.callback([{ target, contentRect: { width, height, top: 0, left: 0, bottom: height, right: width, x: 0, y: 0, toJSON: () => ({}) } } as ResizeObserverEntry], this as unknown as ResizeObserver);
    }
    unobserve() {}
    disconnect() {}
  }
  window.ResizeObserver = MockResizeObserver as unknown as typeof ResizeObserver;
  globalThis.ResizeObserver = MockResizeObserver as unknown as typeof ResizeObserver;
  return () => {
    window.ResizeObserver = original;
    globalThis.ResizeObserver = original;
  };
}

const zoomText = () => screen.getAllByRole("status")[0]!;

describe("selected-object topology", () => {
  it.each([
    { statuses: ["disabled"] as const, dash: "7 5", labels: ["Disabled"] },
    { statuses: ["draft"] as const, dash: "2 4", labels: ["Draft"] },
    { statuses: ["active", "disabled"] as const, dash: "7 3 2 3", labels: ["Active", "Disabled"] },
    { statuses: ["draft", "disabled"] as const, dash: "7 3 2 3", labels: ["Draft", "Disabled"] },
  ])("shows effective $statuses on graph edges and in the bucket", ({ statuses, dash, labels }) => {
    const flows = statuses.map((effective_status, index) => ({ ...flow, id: `status-${index}`, name: `Flow ${index}`, effective_status }));
    render(<DataflowTopology flows={flows} onInspect={vi.fn()} />);
    const canvas = screen.getByRole("region", { name: en.dataflows.topologyCanvas });
    expect(canvas.querySelector('path[pointer-events="none"]')).toHaveAttribute("stroke-dasharray", dash);
    fireEvent.click(screen.getByRole("button", { name: `Shared host → Database: ${flows.length} flows` }));
    const bucket = screen.getByRole("region", { name: en.dataflows.bucketTitle });
    for (const label of labels) expect(within(bucket).getByText(label)).toBeInTheDocument();
  });

  it("centres the most connected object with separate incoming and outgoing lanes", () => {
    const hub = makeNode("hub", "Hub");
    const flows = [
      makeFlow(makeNode("in-1", "Input 1"), hub, "i1", "Input flow 1"),
      makeFlow(makeNode("in-2", "Input 2"), hub, "i2", "Input flow 2"),
      makeFlow(hub, makeNode("out-1", "Output 1"), "o1", "Output flow 1"),
      makeFlow(hub, makeNode("out-2", "Output 2"), "o2", "Output flow 2"),
    ];
    render(<DataflowTopology flows={flows} onInspect={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Highlight connections for Hub" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(en.dataflows.incomingTitle)).toBeInTheDocument();
    expect(screen.getByText(en.dataflows.outgoingTitle)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Input 1 → Hub: 1 flows" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Hub → Output 1: 1 flows" })).toBeInTheDocument();
  });

  it("keeps reciprocal and self links distinct and drills into their own flows", () => {
    const hub = makeNode("hub", "Hub");
    const peer = makeNode("peer", "Peer");
    const flows = [
      makeFlow(hub, peer, "first", "First"),
      makeFlow(peer, hub, "reverse", "Reverse"),
      makeFlow(peer, hub, "parallel", "Parallel"),
      makeFlow(hub, hub, "self", "Self"),
    ];
    render(<DataflowTopology flows={flows} onInspect={vi.fn()} />);
    const incoming = screen.getByRole("button", { name: "Peer → Hub: 2 flows" });
    const outgoing = screen.getByRole("button", { name: "Hub → Peer: 1 flows" });
    const self = screen.getByRole("button", { name: i18n.t("dataflows.internalFlowLabel", { name: "Hub", count: 1 }) });
    expect(screen.getByText(en.dataflows.internalFlow)).toBeInTheDocument();

    fireEvent.click(incoming);
    expect(screen.getByRole("button", { name: "Inspect Reverse" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Inspect Parallel" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect First" })).not.toBeInTheDocument();

    fireEvent.click(outgoing);
    expect(screen.getByRole("button", { name: "Inspect First" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect Reverse" })).not.toBeInTheDocument();

    fireEvent.click(self);
    expect(screen.getByRole("button", { name: "Inspect Self" })).toBeInTheDocument();

    const peerCards = screen.getAllByRole("button", { name: "Highlight connections for Peer" });
    expect(peerCards).toHaveLength(2);
    fireEvent.click(peerCards[0]!);
    expect(screen.getByRole("button", { name: "Highlight connections for Peer" })).toHaveAttribute("aria-pressed", "true");
  });

  it.each([521, 1000])("searches every graph node and finds the last of %i", count => {
    const flows = Array.from({ length: count }, (_, index) => makeFlow(makeNode(`node-${index}`, `Node ${index}`), flow.destinations[0]!, `f-${index}`, `Flow ${index}`));
    render(<DataflowTopology flows={flows} onInspect={vi.fn()} />);
    const search = screen.getByRole("textbox", { name: en.dataflows.findNode });
    fireEvent.change(search, { target: { value: "Node" } });
    expect(within(screen.getByRole("list", { name: en.dataflows.findNode })).getAllByRole("button")).toHaveLength(50);
    expect(screen.getByText(i18n.t("dataflows.topologySearchMore", { shown: 50, total: count }))).toBeInTheDocument();

    fireEvent.change(search, { target: { value: `Node ${count - 1}` } });
    fireEvent.click(screen.getByRole("button", { name: `Node ${count - 1} · node-${count - 1}` }));
    expect(screen.getByRole("button", { name: `Highlight connections for Node ${count - 1}` })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: `Inspect Flow ${count - 1}` })).toBeInTheDocument();
  });

  it("pages through all twelve neighbours while keeping every incident flow searchable", () => {
    const hub = makeNode("hub", "Hub");
    const flows = Array.from({ length: 12 }, (_, index) => makeFlow(hub, makeNode(`peer-${index}`, `Peer ${index}`), `flow-${index}`, `Flow ${index}`));
    render(<DataflowTopology flows={flows} onInspect={vi.fn()} />);
    const seen = new Set<string>();
    for (let page = 0; page < 3; page++) {
      for (const button of screen.getAllByRole("button", { name: /^Highlight connections for Peer / })) seen.add(button.getAttribute("aria-label")!);
      expect(screen.getAllByRole("button", { name: /^Inspect / })).toHaveLength(12);
      if (page < 2) fireEvent.click(screen.getByRole("button", { name: en.dataflows.nextNeighbors }));
    }
    expect(seen.size).toBe(12);
    const previous = screen.getByRole("button", { name: en.dataflows.previousNeighbors });
    expect(previous).toBeEnabled();
    fireEvent.click(previous);
    expect(screen.getByText(/^2 \/ 3$/)).toBeInTheDocument();
  });

  it("auto-fits on entry and after a centre change while manual zoom survives an unrelated rerender", () => {
    render(<DataflowTopology flows={[flow]} onInspect={vi.fn()} />);
    expect(zoomText()).toHaveTextContent("100%");
    expect(scrollTo).toHaveBeenCalledWith({ left: 0, top: 0 });

    fireEvent.click(screen.getByRole("button", { name: en.dataflows.zoomIn }));
    expect(zoomText()).toHaveTextContent("150%");
    fireEvent.change(screen.getByRole("textbox", { name: en.dataflows.findNode }), { target: { value: "Shared" } });
    expect(zoomText()).toHaveTextContent("150%");

    scrollTo.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Highlight connections for Database" }));
    expect(zoomText()).toHaveTextContent("100%");
    expect(scrollTo).toHaveBeenCalledWith({ left: 0, top: 0 });
  });

  it("supports keyboard pan, bounded zoom, fit and reset", () => {
    render(<DataflowTopology flows={[flow]} onInspect={vi.fn()} />);
    const canvas = screen.getByRole("region", { name: en.dataflows.topologyCanvas });
    expect(canvas).toHaveAttribute("tabindex", "0");

    canvas.scrollLeft = 120;
    fireEvent.keyDown(canvas, { key: "ArrowRight" });
    expect(canvas.scrollLeft).toBe(180);
    fireEvent.keyDown(canvas, { key: "ArrowLeft" });
    expect(canvas.scrollLeft).toBe(120);
    canvas.scrollTop = 120;
    fireEvent.keyDown(canvas, { key: "ArrowDown" });
    expect(canvas.scrollTop).toBe(180);
    fireEvent.keyDown(canvas, { key: "ArrowUp" });
    expect(canvas.scrollTop).toBe(120);

    for (let index = 0; index < 5; index++) fireEvent.click(screen.getByRole("button", { name: en.dataflows.zoomOut }));
    expect(zoomText()).toHaveTextContent("50%");
    expect(screen.getByRole("button", { name: en.dataflows.zoomOut })).toBeDisabled();

    fireEvent.click(screen.getByRole("button", { name: en.dataflows.fitGraph }));
    expect(zoomText()).toHaveTextContent("100%");

    canvas.scrollLeft = 300;
    canvas.scrollTop = 200;
    fireEvent.keyDown(canvas, { key: "0" });
    expect(canvas.scrollLeft).toBe(0);
    expect(canvas.scrollTop).toBe(0);
  });

  it("expands measured cards for long labels without clamping them", () => {
    const longName = "A long human-readable endpoint name with words ".repeat(8) + "unbroken-identifier".repeat(8);
    const original = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "scrollHeight");
    Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
      configurable: true,
      get(this: HTMLElement) { return this.textContent?.includes(longName) ? 240 : 40; },
    });
    try {
      render(<DataflowTopology flows={[{ ...flow, sources: [{ ...flow.sources[0]!, label: longName }] }]} onInspect={vi.fn()} />);
      const node = screen.getByRole("button", { name: `Highlight connections for ${longName}` });
      expect(node.parentElement!.parentElement).toHaveAttribute("height", "264");
      const label = within(node).getByText(longName);
      expect(label).toHaveClass("break-words");
      expect(label).not.toHaveClass("line-clamp-2");
    } finally {
      if (original) Object.defineProperty(HTMLElement.prototype, "scrollHeight", original);
      else delete (HTMLElement.prototype as { scrollHeight?: unknown }).scrollHeight;
    }
  });

  it("renders a readable single column on a 390px viewport without SVG scaling", () => {
    const restore = mockResizeObserver(390, 560);
    viewportWidth = 390;
    viewportHeight = 560;
    try {
      const hub = makeNode("hub", "Hub");
      const peer = makeNode("peer", "Peer");
      const flows = [makeFlow(peer, hub, "in", "Input flow"), makeFlow(hub, peer, "out", "Output flow"), makeFlow(hub, hub, "self", "Self flow")];
      const { container } = render(<DataflowTopology flows={flows} onInspect={vi.fn()} />);
      const canvas = screen.getByRole("region", { name: en.dataflows.topologyCanvas });
      expect(canvas.querySelector("foreignObject")).toBeNull();
      expect(canvas.querySelector("marker")).toBeNull();
      expect(container.querySelector("svg[viewBox='0 0 10 10']")).toBeNull();
      expect(canvas).toHaveClass("overflow-auto");
      expect(within(canvas).getByRole("heading", { name: en.dataflows.incomingTitle })).toBeInTheDocument();
      expect(within(canvas).getByRole("heading", { name: en.dataflows.outgoingTitle })).toBeInTheDocument();
      expect(within(canvas).getByRole("heading", { name: en.dataflows.internalFlow })).toBeInTheDocument();
      expect(within(canvas).getByRole("button", { name: "Peer → Hub: 1 flows" })).toBeInTheDocument();
      expect(within(canvas).getByRole("button", { name: "Hub → Peer: 1 flows" })).toBeInTheDocument();
      expect(within(canvas).getByRole("button", { name: i18n.t("dataflows.internalFlowLabel", { name: "Hub", count: 1 }) })).toBeInTheDocument();
      const peerCard = within(canvas).getAllByRole("button", { name: "Highlight connections for Peer" })[0]!;
      const label = within(peerCard).getByText("Peer");
      expect(label).toHaveClass("text-sm");
      expect(label).not.toHaveClass("truncate");
    } finally {
      restore();
    }
  });
});

describe("matrix", () => {
  it("keeps the matrix bounded and drills through all 1000 originals", () => {
    const inspect = vi.fn();
    const flows = Array.from({ length: 1000 }, (_, index) => ({ ...flow, id: `f-${index}`, name: `Flow ${index}`, sources: [{ ...flow.sources[0]!, object_id: `host-${index}` }], destinations: [{ ...flow.destinations[0]!, object_type: "ipam.service" as const }] }));
    const model = buildDataflowVisualization(flows);
    render(<DataflowMatrix graph={model.matrix} onInspect={inspect} />);
    expect(screen.getAllByRole("cell")).toHaveLength(4);
    fireEvent.click(screen.getByRole("button", { name: "Device → Service: 1000 flows" }));
    expect(screen.getAllByRole("button", { name: /^Inspect/ })).toHaveLength(20);
    fireEvent.click(screen.getByRole("button", { name: en.dataflows.nextBucket }));
    expect(screen.getByRole("button", { name: "Inspect Flow 20" })).toBeInTheDocument();
    fireEvent.change(screen.getByRole("textbox", { name: en.dataflows.bucketSearch }), { target: { value: "host-999" } });
    fireEvent.click(screen.getByRole("button", { name: "Inspect Flow 999" }));
    expect(inspect.mock.calls[0]?.[0].id).toBe("f-999");
  });

  it("keeps reverse and self matrix cells distinct and drills down to their own originals", () => {
    const internet = { ...flow.destinations[0]!, object_type: "internet" as const, object_id: null };
    const flows = [{ ...flow, destinations: [internet] }, { ...flow, id: "reverse", name: "Reverse", sources: [internet], destinations: [flow.sources[0]!] }, { ...flow, id: "self", name: "Self", destinations: [flow.sources[0]!] }];
    render(<DataflowMatrix graph={buildDataflowVisualization(flows).matrix} onInspect={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Device → Internet / any IP: 1 flows" }));
    expect(screen.getByRole("button", { name: "Inspect First" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect Reverse" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Internet / any IP → Device: 1 flows" }));
    expect(screen.getByRole("button", { name: "Inspect Reverse" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Inspect First" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Device → Device: 1 flows" }));
    expect(screen.getByRole("button", { name: "Inspect Self" })).toBeInTheDocument();
  });
});
