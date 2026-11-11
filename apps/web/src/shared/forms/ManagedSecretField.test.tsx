import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { fetchSecretFieldStates } from "@/shared/settings/api";
import ManagedSecretField from "./ManagedSecretField";

vi.mock("@/shared/settings/api", () => ({ fetchSecretFieldStates: vi.fn() }));
const reveal = vi.fn();
const props = { label: "Authentication Key", consumerType: "inventory_fhrp_group", consumerId: "group-id",
  fieldKey: "auth_key", intent: "", value: "", canReveal: true, onIntent: vi.fn(), onValue: vi.fn(), reveal };
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(fetchSecretFieldStates).mockResolvedValue([{ field_key: "auth_key", has_value: true,
    consumer_id: "group-id", consumer_type: "inventory_fhrp_group", enabled: true,
    owner_scope_id: "group-id", owner_scope_type: "inventory_fhrp_group" }]);
});

describe("managed secret editor", () => {
  it("does not offer reveal without permission", async () => {
    render(<ManagedSecretField {...props} canReveal={false} />);
    expect(await screen.findByText("Key configured")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reveal" })).not.toBeInTheDocument();
    expect(reveal).not.toHaveBeenCalled();
  });
  it("loads metadata only, then reveals explicitly without dirtying the draft", async () => {
    reveal.mockResolvedValue("synthetic-reveal");
    render(<ManagedSecretField {...props} />);
    const button = await screen.findByRole("button", { name: "Reveal" });
    expect(fetchSecretFieldStates).toHaveBeenCalledWith("inventory_fhrp_group", "group-id");
    expect(reveal).not.toHaveBeenCalled();
    expect(screen.queryByText("synthetic-reveal")).not.toBeInTheDocument();
    fireEvent.click(button);
    expect(await screen.findByText("synthetic-reveal")).toBeInTheDocument();
    expect(props.onValue).not.toHaveBeenCalled();
    expect(props.onIntent).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByText("synthetic-reveal")).not.toBeInTheDocument();
  });

  it("keeps a denied reveal masked and does not alter the pending edit", async () => {
    reveal.mockRejectedValue(new Error("Forbidden"));
    render(<ManagedSecretField {...props} />);
    fireEvent.click(await screen.findByRole("button", { name: "Reveal" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The key could not be revealed");
    expect(screen.queryByRole("status")).not.toHaveTextContent("Forbidden");
    expect(props.onValue).not.toHaveBeenCalled();
    expect(props.onIntent).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Reveal" })).not.toBeInTheDocument();
  });
});
