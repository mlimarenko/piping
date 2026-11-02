import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TogglePill, TogglePillGroup } from "@/shared/ui-shared/TogglePill";

describe("TogglePill", () => {
  it("reflects active state via aria-pressed and fires onClick", () => {
    const onClick = vi.fn();
    render(
      <TogglePill active onClick={onClick}>
        Allow
      </TogglePill>,
    );
    const pill = screen.getByRole("button", { name: "Allow" });
    expect(pill).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(pill);
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});

describe("TogglePillGroup", () => {
  it("marks only the selected option pressed and emits the chosen value", () => {
    const onValueChange = vi.fn();
    render(
      <TogglePillGroup
        value="allow"
        onValueChange={onValueChange}
        options={[
          { value: "allow", label: "Allow" },
          { value: "deny", label: "Deny" },
        ]}
      />,
    );
    expect(screen.getByRole("button", { name: "Allow" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Deny" })).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(screen.getByRole("button", { name: "Deny" }));
    expect(onValueChange).toHaveBeenCalledWith("deny");
  });
});
