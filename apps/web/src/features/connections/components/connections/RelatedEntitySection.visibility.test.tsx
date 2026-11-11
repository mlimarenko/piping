import { render, screen } from "@testing-library/react";
import { Users } from "lucide-react";
import { describe, expect, it, vi } from "vitest";
import RelatedEntitySection from "./RelatedEntitySection";

const props = {
  title: "Contacts",
  icon: Users,
  sectionKey: "contacts",
  expandedSections: ["contacts"],
  onToggle: vi.fn(),
  items: [] as { id: string }[],
  loading: false,
  count: 0,
  emptyText: "No contacts",
  renderRow: (item: { id: string }) => <span key={item.id}>{item.id}</span>,
  t: (key: string) => key,
};

describe("inspector relation visibility", () => {
  it("hides the entire section only after an authoritative empty response", () => {
    const { rerender } = render(<RelatedEntitySection {...props} count={undefined} countLoading />);
    expect(screen.getByRole("button", { name: /Contacts/ })).toBeInTheDocument();
    rerender(<RelatedEntitySection {...props} />);
    expect(screen.queryByRole("button", { name: /Contacts/ })).not.toBeInTheDocument();
    expect(screen.queryByText("No contacts")).not.toBeInTheDocument();
  });

  it("does not hide loading or errors, including denied reads, as empty", () => {
    const { rerender } = render(<RelatedEntitySection {...props} loading />);
    expect(screen.getByRole("button", { name: /Contacts/ })).toBeInTheDocument();
    rerender(<RelatedEntitySection {...props} error={new Error("Forbidden")} />);
    expect(screen.getByRole("button", { name: /Contacts/ })).toBeInTheDocument();
    expect(screen.getByText("connections.inspector.relations.error")).toBeInTheDocument();
  });

  it("keeps loaded items visible if the independent count is stale", () => {
    render(<RelatedEntitySection {...props} items={[{ id: "existing-contact" }]} />);
    expect(screen.getByText("existing-contact")).toBeInTheDocument();
  });

  it("announces a failed count in the collapsed header instead of spinning forever", () => {
    const { container } = render(<RelatedEntitySection {...props} count={undefined} countLoading
      expandedSections={[]} error={new Error("Denied")} />);
    expect(screen.getByRole("img", { name: "connections.inspector.relations.error" })).toBeInTheDocument();
    expect(container.querySelector(".animate-spin")).toBeNull();
    expect(screen.getByRole("button", { name: /Contacts/ })).toHaveAttribute("aria-expanded", "false");
  });
});
