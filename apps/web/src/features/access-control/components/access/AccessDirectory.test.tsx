import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";
import AccessDirectory from "./AccessDirectory";

// The four inner surfaces are covered by their own suites; here we only assert
// the wrapper mounts the right one per section and switches via route links.
vi.mock("./AccessUsers", () => ({ default: () => <div data-testid="pane-users" /> }));
vi.mock("./AccessGroups", () => ({ default: () => <div data-testid="pane-groups" /> }));
vi.mock("./AccessRoles", () => ({ default: () => <div data-testid="pane-roles" /> }));
vi.mock("./AccessGrants", () => ({ default: () => <div data-testid="pane-grants" /> }));

const renderDir = (initialSection?: "users" | "groups" | "roles" | "grants") =>
  render(
    <AllPermissions>
      <MemoryRouter>
        <AccessDirectory initialSection={initialSection} />
      </MemoryRouter>
    </AllPermissions>,
  );

describe("AccessDirectory", () => {
  it("defaults to the Users section", () => {
    renderDir();
    expect(screen.getByTestId("pane-users")).toBeInTheDocument();
    expect(screen.queryByTestId("pane-roles")).toBeNull();
  });

  it("switches sections via route links and unmounts the previous panel", () => {
    renderDir();
    const rolesLink = screen.getByRole("link", { name: /roles/i });
    expect(rolesLink).toHaveAttribute("href", "/?tab=directory&section=roles");
    fireEvent.click(rolesLink);
    expect(screen.getByTestId("pane-roles")).toBeInTheDocument();
    expect(screen.queryByTestId("pane-users")).toBeNull();
    expect(rolesLink).toHaveAttribute("aria-current", "page");
  });

  it("honours initialSection", () => {
    renderDir("grants");
    expect(screen.getByTestId("pane-grants")).toBeInTheDocument();
  });
});
