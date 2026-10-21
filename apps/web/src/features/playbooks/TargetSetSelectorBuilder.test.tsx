import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import type { ConnectionTarget } from "@/shared/api/types";
import { testUuid } from "@/test/ids";
import TargetSetSelectorBuilder from "./TargetSetSelectorBuilder";
import { emptySelectorModel } from "./selectorModel";

const target = (overrides: Partial<ConnectionTarget>): ConnectionTarget => ({
  id: testUuid("11111111-1111-1111-1111-111111111111"),
  name: "router-1",
  targetKind: "device",
  ...overrides,
});

describe("TargetSetSelectorBuilder", () => {
  it("keeps stale selected role values removable when inventory no longer exposes them", () => {
    const onChange = vi.fn();
    const model = emptySelectorModel();
    model.facets.role = ["Live role", "Deleted role"];

    render(
      <TargetSetSelectorBuilder
        model={model}
        onChange={onChange}
        targets={[target({ role: "Live role" })]}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /Role 2/i }));

    expect(screen.getByRole("button", { name: /Deleted role missing/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Deleted role missing/i }));

    expect(onChange).toHaveBeenCalledWith({
      ...model,
      facets: {
        ...model.facets,
        role: ["Live role"],
      },
    });
  });
});
