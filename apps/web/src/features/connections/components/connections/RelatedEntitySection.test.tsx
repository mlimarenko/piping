import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { Server } from "lucide-react";
import type { ReactElement } from "react";
import { describe, expect, it, vi } from "vitest";
import RelatedEntitySection, { RelatedEntityRow } from "./RelatedEntitySection";

const t = (key: string) => key;

interface Item {
  id: string;
  name: string;
}

const items: Item[] = [
  { id: "a", name: "alpha" },
  { id: "b", name: "beta" },
];

const renderRow = (item: Item) => <RelatedEntityRow key={item.id} icon={Server} label={item.name} />;

function renderInRouter(ui: ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

function baseProps(overrides: Partial<Parameters<typeof RelatedEntitySection<Item>>[0]> = {}) {
  return {
    title: "Devices",
    icon: Server,
    sectionKey: "devices",
    // Expanded by default so the body renders without a toggle click.
    expandedSections: ["devices"],
    onToggle: vi.fn(),
    items,
    loading: false,
    renderRow,
    emptyText: "No devices",
    t,
    ...overrides,
  };
}

describe("RelatedEntitySection", () => {
  it("renders a count badge and one row per item", () => {
    renderInRouter(<RelatedEntitySection {...baseProps()} />);
    expect(screen.getByText("alpha")).toBeInTheDocument();
    expect(screen.getByText("beta")).toBeInTheDocument();
    // Count badge reflects items length.
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("uses the explicit count override over items.length", () => {
    renderInRouter(<RelatedEntitySection {...baseProps({ count: 42 })} />);
    expect(screen.getByText("42")).toBeInTheDocument();
  });

  it("shows a spinner (never a premature '0') while the eager count is still loading", () => {
    const { container } = renderInRouter(
      <RelatedEntitySection {...baseProps({ items: [], count: undefined, countLoading: true })} />,
    );
    expect(screen.queryByText("0")).toBeNull();
    expect(container.querySelector("svg.animate-spin")).not.toBeNull();
  });

  it("shows '0' once the eager count resolves to an empty relation", () => {
    renderInRouter(<RelatedEntitySection {...baseProps({ items: [], count: 0, countLoading: false })} />);
    expect(screen.getByText("0")).toBeInTheDocument();
  });

  it("shows the empty text when there are no items", () => {
    renderInRouter(<RelatedEntitySection {...baseProps({ items: [] })} />);
    expect(screen.getByText("No devices")).toBeInTheDocument();
  });

  it("shows the shared error string when error is set", () => {
    renderInRouter(<RelatedEntitySection {...baseProps({ error: new Error("boom") })} />);
    expect(screen.getByText("connections.inspector.relations.error")).toBeInTheDocument();
    // Rows are not rendered in the error state.
    expect(screen.queryByText("alpha")).toBeNull();
  });

  it("shows a skeleton (no rows, no empty text) while loading", () => {
    renderInRouter(<RelatedEntitySection {...baseProps({ loading: true })} />);
    expect(screen.queryByText("alpha")).toBeNull();
    expect(screen.queryByText("No devices")).toBeNull();
  });

  it("never renders a delete/detach button", () => {
    renderInRouter(<RelatedEntitySection {...baseProps()} />);
    // No destructive affordance anywhere — detach/delete lives in the edit form.
    expect(screen.queryByRole("button", { name: /remove|delete|detach/i })).toBeNull();
  });

  it("renders the header + button and calls onAdd when clicked", () => {
    const onAdd = vi.fn();
    renderInRouter(<RelatedEntitySection {...baseProps({ onAdd, addLabel: "Add device" })} />);
    fireEvent.click(screen.getByRole("button", { name: "Add device" }));
    expect(onAdd).toHaveBeenCalledTimes(1);
  });

  it("exposes exactly one + per section (header only, even when empty)", () => {
    const onAdd = vi.fn();
    renderInRouter(
      <RelatedEntitySection {...baseProps({ items: [], onAdd, addLabel: "Add device" })} />,
    );
    // Single add affordance — the empty body shows text only, not a second button.
    const adds = screen.getAllByRole("button", { name: "Add device" });
    expect(adds).toHaveLength(1);
    fireEvent.click(adds[0]!);
    expect(onAdd).toHaveBeenCalledTimes(1);
  });

  it("omits the + button entirely when onAdd is undefined", () => {
    renderInRouter(<RelatedEntitySection {...baseProps()} />);
    expect(screen.queryByRole("button", { name: /add/i })).toBeNull();
  });

  it("renders the body only when the section is expanded", () => {
    renderInRouter(<RelatedEntitySection {...baseProps({ expandedSections: [] })} />);
    expect(screen.queryByText("alpha")).toBeNull();
  });
});

describe("RelatedEntityRow", () => {
  it("links the whole row when an href is present", () => {
    renderInRouter(
      <RelatedEntityRow icon={Server} label="srv-1" href="/inventory/resources?entity=Devices" />,
    );
    expect(screen.getByRole("link", { name: "srv-1" })).toHaveAttribute(
      "href",
      "/inventory/resources?entity=Devices",
    );
  });

  it("renders an inert row (no link, no nested interactive elements) without an href", () => {
    renderInRouter(
      <RelatedEntityRow icon={Server} label="srv-2" secondary1="role" secondary2="active" />,
    );
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getByText("srv-2")).toBeInTheDocument();
    // Secondary fields render as plain joined muted text — not separate links.
    expect(screen.getByText("role · active")).toBeInTheDocument();
  });
});
