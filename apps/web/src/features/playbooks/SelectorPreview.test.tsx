import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { SelectorPreview } from "./SelectorPreview";
import { emptySelectorModel } from "./selectorModel";
import type { PlaybookTargetSelectorPreview } from "./api";

const data: PlaybookTargetSelectorPreview = {
  total: 100, limit: 50, truncated: true,
  items: [{ target_kind: "connection_target", target_id: "id", source_kind: "device", source_id: "source", name: "Router", address: "192.0.2.1", address_bound: true, role_name: "Edge", role_id: "role", status: "active" }],
};

describe("selector preview", () => {
  it("shows matching field values and an explicit truncated sample", () => {
    const model = emptySelectorModel();
    model.facets.role = ["role"];
    render(<SelectorPreview data={data} model={model} pending={false} failed={false} retry={vi.fn()} />);
    expect(screen.getByText("Router")).toBeInTheDocument();
    expect(screen.getByText("Role:")).toBeInTheDocument();
    expect(screen.getByText("Edge")).toBeInTheDocument();
    expect(screen.getByText(/Showing the first 1 hosts/)).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("100 hosts");
  });

  it("hides an old sample while updating and on error, with retry instead of an invented zero", () => {
    const retry = vi.fn();
    const props = { data, model: emptySelectorModel(), retry };
    const view = render(<SelectorPreview {...props} pending failed={false} />);
    expect(screen.queryByText("Router")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Updating");
    view.rerender(<SelectorPreview {...props} pending={false} failed />);
    expect(screen.queryByText("Router")).not.toBeInTheDocument();
    expect(screen.queryByText("No hosts match these conditions.")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Retry/i }));
    expect(retry).toHaveBeenCalledOnce();
  });
});
