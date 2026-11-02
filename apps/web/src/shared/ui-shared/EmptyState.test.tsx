import { fireEvent, render, screen } from "@testing-library/react";
import { Database, RefreshCw } from "lucide-react";
import { describe, expect, it, vi } from "vitest";
import { Button } from "@/shared/ui-primitives/button";
import EmptyState from "./EmptyState";

describe("EmptyState composition", () => {
  it("renders a retry icon instead of the create icon and invokes retry", () => {
    const retry = vi.fn();
    render(<EmptyState icon={Database} title="Unavailable" actionLabel="Retry" onAction={retry} actionIcon={RefreshCw} />);
    const button = screen.getByRole("button", { name: "Retry" });
    expect(button.querySelector(".lucide-refresh-cw")).toHaveAttribute("aria-hidden", "true");
    expect(button.querySelector(".lucide-plus")).toBeNull();
    fireEvent.click(button);
    expect(retry).toHaveBeenCalledOnce();
  });

  it("renders body content and multiple independent actions", () => {
    const source = vi.fn();
    const group = vi.fn();
    render(<EmptyState icon={Database} title="No profiles" actions={<>
      <Button variant="outline" onClick={source}>Assign source</Button>
      <Button onClick={group}>Assign group</Button>
    </>}><p>Select the scope first.</p></EmptyState>);
    expect(screen.getByText("Select the scope first.")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Assign source" }));
    fireEvent.click(screen.getByRole("button", { name: "Assign group" }));
    expect(source).toHaveBeenCalledOnce();
    expect(group).toHaveBeenCalledOnce();
  });

  it("keeps the simple create convenience and omits absent actions", () => {
    const view = render(<EmptyState icon={Database} title="No sources" actionLabel="Create" onAction={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Create" }).querySelector(".lucide-plus")).not.toBeNull();
    view.rerender(<EmptyState icon={Database} title="No sources" />);
    expect(screen.queryByRole("button")).toBeNull();
  });
});
