import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import {
  Command,
  CommandInput,
  CommandList,
} from "@/shared/ui-primitives/command";

describe("Command primitives", () => {
  it("keeps search text padded away from the input edge", () => {
    render(
      <Command>
        <CommandInput placeholder="Search records" />
      </Command>,
    );

    expect(screen.getByPlaceholderText("Search records")).toHaveClass("px-1.5");
  });

  it("uses a native wheel-scrollable list surface", () => {
    render(
      <Command>
        <CommandList>
          <div>row</div>
        </CommandList>
      </Command>,
    );

    const list = document.querySelector("[cmdk-list]");
    expect(list).toHaveClass(
      "overflow-y-auto",
      "overscroll-contain",
      "stable-scrollbar-gutter",
    );
  });
});
