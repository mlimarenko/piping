import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { render } from "@/test/pageChrome";
import "@/shared/i18n";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { RequireCapability } from "./permissions";
import { PermissionsContext, type PermissionsValue } from "./permissionsContext";

const permissions = (allowed: boolean): PermissionsValue => ({
  roles: [], isAdmin: false, can: () => allowed, canAny: () => allowed, hasRole: () => false,
});

describe("guarded route chrome", () => {
  it("replaces page actions with an explicit denied title when access is removed", () => {
    const page = (allowed: boolean) => <PermissionsContext.Provider value={permissions(allowed)}>
      <RequireCapability check={value => value.can("READ", "INVENTORY")}>
        <PageChrome title="Resources" actions={<button>Create resource</button>} />
        <div>Resource content</div>
      </RequireCapability>
    </PermissionsContext.Provider>;
    const view = render(page(true));
    expect(within(screen.getByRole("banner")).getByRole("heading", { level: 1 })).toHaveTextContent("Resources");
    expect(screen.getByRole("button", { name: "Create resource" })).toBeInTheDocument();
    view.rerender(page(false));
    expect(within(screen.getByRole("banner")).getByRole("heading", { level: 1 })).not.toHaveTextContent("Resources");
    expect(screen.queryByRole("button", { name: "Create resource" })).toBeNull();
    expect(screen.queryByText("Resource content")).toBeNull();
    expect(screen.queryByRole("status")).toBeNull();
    view.rerender(page(true));
    expect(within(screen.getByRole("banner")).getByRole("heading", { level: 1 })).toHaveTextContent("Resources");
    expect(screen.getByRole("button", { name: "Create resource" })).toBeInTheDocument();
  });
});
