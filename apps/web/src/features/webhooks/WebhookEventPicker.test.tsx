import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import WebhookEventPicker from "./WebhookEventPicker";

const catalog = [
  { group: "inventory", object: "host", action: "created", value: "inventory.host.created" },
  { group: "inventory", object: "host", action: "updated", value: "inventory.host.updated" },
];

describe("WebhookEventPicker", () => {
  it("selects events and filters the catalog by search", () => {
    const onChange = vi.fn();
    render(<WebhookEventPicker catalog={catalog} loading={false} value={[]} onChange={onChange} />);

    const created = screen.getByRole("checkbox", { name: "inventory.host.created" });
    fireEvent.click(created);
    expect(onChange).toHaveBeenCalledWith(["inventory.host.created"]);

    const search = screen.getByRole("textbox");
    fireEvent.change(search, { target: { value: "updated" } });
    expect(screen.queryByRole("checkbox", { name: "inventory.host.created" })).toBeNull();
    expect(screen.getByRole("checkbox", { name: "inventory.host.updated" })).toBeInTheDocument();
  });
});
