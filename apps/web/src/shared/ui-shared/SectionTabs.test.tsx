import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import SectionTabs from "./SectionTabs";

describe("SectionTabs", () => {
  it.each(["line", "secondary"] as const)("preserves keyboard navigation and panel relationships in %s tabs", async (variant) => {
    render(
      <Tabs defaultValue="users">
        <SectionTabs variant={variant} items={[
          { value: "users", label: "Users" },
          { value: "groups", label: "Groups" },
        ]} />
        <TabsContent value="users">User directory</TabsContent>
        <TabsContent value="groups">Group directory</TabsContent>
      </Tabs>,
    );

    expect(screen.getByRole("tablist")).toHaveAttribute("data-variant", variant);
    const users = screen.getByRole("tab", { name: "Users" });
    const groups = screen.getByRole("tab", { name: "Groups" });
    expect(users).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel")).toHaveAttribute("aria-labelledby", users.id);

    act(() => users.focus());
    fireEvent.keyDown(users, { key: "ArrowRight" });
    await waitFor(() => expect(groups).toHaveFocus());
    expect(groups).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel")).toHaveTextContent("Group directory");
    expect(screen.getByRole("tabpanel")).toHaveAttribute("id", groups.getAttribute("aria-controls"));

    fireEvent.keyDown(groups, { key: "Home" });
    await waitFor(() => expect(users).toHaveFocus());
    expect(screen.getByRole("tabpanel")).toHaveTextContent("User directory");
  });

  it("forwards the tablist accessible name and standard TabsList attributes", () => {
    render(
      <Tabs defaultValue="users">
        <SectionTabs
          aria-label="Directory sections"
          data-testid="section-tablist"
          items={[
            { value: "users", label: "Users" },
            { value: "groups", label: "Groups" },
          ]}
        />
        <TabsContent value="users">User directory</TabsContent>
      </Tabs>,
    );

    const tablist = screen.getByRole("tablist", { name: "Directory sections" });
    expect(tablist).toHaveAttribute("data-testid", "section-tablist");
    expect(tablist).toHaveAttribute("data-variant", "line");
  });
});
