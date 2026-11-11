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

describe("AppSidebar automation navigation", () => {
  it("selects automation for a group reader", () => {
    renderScopedSidebar(new Set(["PLAYBOOK_RUN_GROUP"]), "/playbooks/runs");
    expect(screen.getByRole("link", { name: "Playbooks" })).toHaveAttribute("aria-current", "page");
  });
  it("does not expose automation to an individual-run-only reader", () => {
    renderScopedSidebar(new Set(["PLAYBOOK_RUN"]), "/playbooks/runs");
    expect(screen.queryByRole("link", { name: "Playbooks" })).toBeNull();
  });
  it("lands on the first surface the user can read", () => {
    renderScopedSidebar(new Set(["PLAYBOOK_SCHEDULE"]), "/playbooks/schedules");
    const link = screen.getByRole("link", { name: "Playbooks" });
    expect(link).toHaveAttribute("href", "/playbooks/schedules");
    expect(link).toHaveAttribute("aria-current", "page");
  });
});

describe("AppSidebar navigation labels", () => {
  it.each(["/playbooks/schedules/new", "/playbooks/settings/target-sets/target-1/edit", "/playbooks/projects/project-1/files"])("selects automation for nested route: %s", path => {
    renderSidebar(path);
    expect(screen.getByRole("button", { name: "Automation" })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("link", { name: "Playbooks" })).toHaveAttribute("aria-current", "page");
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
