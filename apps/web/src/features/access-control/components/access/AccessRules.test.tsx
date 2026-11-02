import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import AccessRules from "./AccessRules";

// The policy list and access-check are covered by their own suites; here we only
// assert the wrapper renders the list and the collapsible Test-access behaviour.
vi.mock("./AccessCheck", () => ({ default: () => <div data-testid="access-check-panel" /> }));
vi.mock("./AccessPolicies", () => ({
  default: () => <div data-testid="policies" />,
}));

describe("AccessRules", () => {
  it("renders the policy list with the Test-access panel collapsed", () => {
    render(<AccessRules />);
    expect(screen.getByTestId("policies")).toBeInTheDocument();
    expect(screen.queryByTestId("access-check-panel")).toBeNull();
  });

  it("toggles the Test-access panel open", () => {
    render(<AccessRules />);
    fireEvent.click(screen.getByRole("button", { name: /test access/i }));
    expect(screen.getByTestId("access-check-panel")).toBeInTheDocument();
  });
});
