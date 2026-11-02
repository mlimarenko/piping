import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import AppSidebar from "./AppSidebar";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";

function scopedPermissions(readable: ReadonlySet<string>): PermissionsValue {
  const can = (action: string, resourceType: string) =>
    action === "READ" && readable.has(resourceType);
  return {
    roles: ["custom"],
    isAdmin: false,
    can,
    canAny: (resourceType, actions) => actions.some((a) => can(a, resourceType)),
    hasRole: (role) => role === "custom",
  };
}

function renderScopedSidebar(readable: ReadonlySet<string>, path = "/playbooks") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <PermissionsContext.Provider value={scopedPermissions(readable)}>
        <AppSidebar collapsed={false} onToggle={vi.fn()} />
      </PermissionsContext.Provider>
    </MemoryRouter>,
  );
}

vi.mock("@/features/assistant", () => ({
  AssistantLauncher: ({ collapsed }: { collapsed: boolean }) => (
    <div data-testid="assistant-launcher" data-collapsed={String(collapsed)} />
  ),
}));

function renderSidebar(path = "/playbooks") {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <AllPermissions>
        <AppSidebar collapsed={false} onToggle={vi.fn()} />
      </AllPermissions>
    </MemoryRouter>,
  );
}

describe("AppSidebar playbook navigation", () => {
  it("shows and selects the group Runs entry for a GROUP-only reader", () => {
    renderScopedSidebar(new Set(["PLAYBOOK_RUN_GROUP"]), "/automation/runs");
    expect(screen.getByRole("link", { name: /runs/i })).toHaveAttribute("aria-current", "page");
  });
  it("hides the group Runs entry for an individual-RUN-only reader", () => {
    renderScopedSidebar(new Set(["PLAYBOOK_RUN"]), "/automation/runs");
    expect(screen.queryByRole("link", { name: /runs/i })).toBeNull();
    expect(screen.getByRole("navigation").querySelector('[aria-current="page"]')).toBeNull();
  });
  it("shows playbooks under automation instead of administration", () => {
    renderSidebar();

    expect(screen.getByRole("button", { name: /automation/i })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /playbooks/i })).toHaveAttribute("href", "/playbooks");

    fireEvent.click(screen.getByRole("button", { name: /administration/i }));

    expect(screen.getAllByText("Playbooks")).toHaveLength(1);
  });

  it("highlights the automation runs route as a sidebar item", () => {
    renderSidebar("/automation/runs");

    const runsLink = screen.getByRole("link", { name: /runs/i });

    expect(runsLink).toHaveAttribute("href", "/automation/runs");
    expect(runsLink).toHaveAttribute("aria-current", "page");
  });
  it("keeps the playbook catalogue active on its new detail route", () => {
    renderSidebar("/playbooks/11111111-1111-4111-8111-111111111111?section=targets");
    expect(screen.getByRole("link", { name: /playbooks/i })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("navigation").querySelectorAll('[aria-current="page"]')).toHaveLength(1);
  });

  it("points Setup at a surface the user can actually read", () => {
    // Operator with only schedule access: Setup must land on /automation/schedules,
    // not the access-denied /automation/projects.
    renderScopedSidebar(new Set(["PLAYBOOK_SCHEDULE", "PLAYBOOK_RUN"]));

    const setupLink = screen.getByRole("link", { name: /setup/i });
    expect(setupLink).toHaveAttribute("href", "/automation/schedules");
  });

  it("keeps Setup lit on any setup surface, not just projects", () => {
    renderScopedSidebar(new Set(["PLAYBOOK_SCHEDULE"]), "/automation/schedules");

    const setupLink = screen.getByRole("link", { name: /setup/i });
    expect(setupLink).toHaveAttribute("aria-current", "page");
  });

  it("hides Setup when no setup surface is readable", () => {
    renderScopedSidebar(new Set(["PLAYBOOK", "PLAYBOOK_RUN"]));

    expect(screen.queryByRole("link", { name: /setup/i })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: /playbooks/i })).toBeInTheDocument();
  });
});

describe("AppSidebar navigation labels", () => {
  it.each(["/automation/schedules/new", "/automation/target-sets/target-1/edit", "/automation/projects/project-1/files"])("selects Setup for nested automation: %s", path => {
    renderSidebar(path);
    expect(screen.getByRole("button", { name: "Automation" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: /setup/i })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("navigation").querySelectorAll('[aria-current="page"]')).toHaveLength(1);
  });
  it("does not select the connection workspace when a nested proxy destination is denied", () => {
    renderScopedSidebar(new Set(["CONNECTION_TARGET"]), "/connections/proxies/node-1/edit");
    expect(screen.getByRole("button", { name: "Connections" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("navigation").querySelector('[aria-current="page"]')).toBeNull();
  });
  it("selects Administration and the proxy catalogue on nested proxy assignments", () => {
    renderSidebar("/connections/proxies/assignments/assignment-1/edit");
    expect(screen.getByRole("button", { name: "Administration" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "Connections" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("link", { name: /proxy nodes/i })).toHaveAttribute("aria-current", "page");
  });
  it("does not confuse a query value prefix with the SSO tab", () => {
    renderScopedSidebar(new Set(["KEYCLOAK"]), "/access-control?tab=sso-extra");
    expect(screen.getByRole("navigation").querySelector('[aria-current="page"]')).toBeNull();
  });
  it("integrates Dataflows into Inventory and selects it on nested editors", () => {
    renderSidebar("/inventory/dataflows/flow-1/edit");
    expect(screen.getByRole("link", { name: "Dataflows" })).toHaveAttribute("href", "/inventory/dataflows");
    expect(screen.getByRole("link", { name: "Dataflows" })).toHaveAttribute("aria-current", "page");
  });
  it("hides Dataflows when Inventory is not readable", () => {
    renderScopedSidebar(new Set(["PLAYBOOK"]), "/inventory/dataflows");
    expect(screen.queryByRole("link", { name: "Dataflows" })).not.toBeInTheDocument();
  });
  it.each(["/connections/config/new", "/connections/config/linux/edit", "/inventory/resources/Devices/new"])("keeps the owning catalogue selected on nested pages: %s", path => {
    renderSidebar(path);
    const active = screen.getByRole("link", {name: path.startsWith("/connections") ? "Connection Config" : "Resources"});
    expect(active).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("navigation").querySelectorAll('[aria-current="page"]')).toHaveLength(1);
  });
  it("only expands the current section on first load", () => {
    renderSidebar("/inventory/resources");
    expect(screen.getByRole("button", { name: "Inventory" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("button", { name: "Connections" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("link", { name: "Resources" })).toHaveAttribute("aria-current", "page");
  });

  it("notifies the mobile shell when a destination is selected", () => {
    const onNavigate = vi.fn();
    render(
      <MemoryRouter initialEntries={["/inventory/resources"]}>
        <AllPermissions><AppSidebar collapsed={false} onToggle={vi.fn()} onNavigate={onNavigate} /></AllPermissions>
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByRole("link", { name: "Resources" }));
    expect(onNavigate).toHaveBeenCalledOnce();
  });

  it("wraps nav labels instead of truncating them (PWA-65)", () => {
    renderSidebar("/connections/config");
    const nav = screen.getByRole("navigation");

    // The reported defect: this item's long label was clipped by `truncate`.
    const item = nav.querySelector('a[href="/connections/config"]');
    expect(item).not.toBeNull();
    const label = item!.querySelector("span");
    expect(label?.textContent).toBeTruthy();
    expect(label).not.toHaveClass("truncate");
    expect(label).toHaveClass("break-words");
    expect(label).toHaveClass("line-clamp-2");
    expect(label!.getAttribute("title")).toBe(label!.textContent);

    const sectionHeaderLabel = nav.querySelector("button[aria-expanded] span");
    expect(sectionHeaderLabel).not.toBeNull();
    expect(sectionHeaderLabel).not.toHaveClass("truncate");
    expect(sectionHeaderLabel).toHaveClass("break-words");

    // No navigation label may fall back to the single-line ellipsis rule.
    for (const span of nav.querySelectorAll("span")) {
      expect(span.className, `"${span.textContent}" must not truncate`).not.toContain("truncate");
    }
  });
});
