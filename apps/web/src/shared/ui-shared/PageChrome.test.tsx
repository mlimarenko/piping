import { createContext, useContext, useState, type ReactNode } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import PageChrome, { PageActions } from "./PageChrome";
import { PageChromeProvider, usePageChromeHosts } from "./pageChromeContext";

function Header() {
  const { setTitleHost, setActionsHost, actionsBlocked } = usePageChromeHosts();
  return <header><div ref={setTitleHost} /><div ref={setActionsHost} inert={actionsBlocked} /></header>;
}
function Shell({ token = "a", children }: { token?: string; children: ReactNode }) {
  return <PageChromeProvider navigationToken={token}><Header /><main>{children}</main></PageChromeProvider>;
}
describe("shell page chrome", () => {
  it("portals one heading and live actions into the header while retaining feature context and focus", () => {
    const Feature = createContext("missing");
    function Action() {
      const value = useContext(Feature);
      const [count, setCount] = useState(0);
      return <button onClick={() => setCount(n => n + 1)}>{value} {count}</button>;
    }
    const view = (title: string) => <Shell><Feature.Provider value="Create"><PageChrome title={title} actions={<Action />} /></Feature.Provider></Shell>;
    const { rerender } = render(view("Resources"));
    const header = screen.getByRole("banner");
    expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Resources");
    expect(within(screen.getByRole("main")).queryByRole("heading")).toBeNull();
    const action = within(header).getByRole("button", { name: "Create 0" });
    action.focus();
    fireEvent.click(action);
    rerender(view("Updated resources"));
    expect(screen.getByRole("button", { name: "Create 1" })).toBe(action);
    expect(action).toHaveFocus();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });
  it("lets the latest owner win and does not let stale cleanup erase it", () => {
    const view = (old: boolean, current: boolean) => <Shell>{old && <PageChrome title="Old" actions={<button>Old action</button>} />}{current && <PageChrome title="Current" actions={<button>Current action</button>} />}</Shell>;
    const { rerender } = render(view(true, false));
    rerender(view(true, true));
    expect(screen.getByRole("heading")).toHaveTextContent("Current");
    expect(screen.queryByRole("button", { name: "Old action" })).toBeNull();
    rerender(view(false, true));
    expect(screen.getByRole("heading")).toHaveTextContent("Current");
    rerender(view(false, false));
    expect(screen.queryByRole("heading")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
  });
  it("clears stale actions on navigation and ignores a retained hidden workspace", () => {
    const view = (token: string, visible: boolean) => <Shell token={token}><PageChrome visible={visible} title="Workspace" actions={<button>Workspace action</button>} />{!visible && <PageChrome title="Editor" />}</Shell>;
    const { rerender } = render(view("a", true));
    expect(screen.getByRole("button", { name: "Workspace action" })).toBeInTheDocument();
    rerender(view("b", false));
    expect(screen.getByRole("heading")).toHaveTextContent("Editor");
    expect(screen.queryByRole("button", { name: "Workspace action" })).toBeNull();
    rerender(<Shell token="c"><div>Loading next page</div></Shell>);
    expect(screen.queryByRole("heading")).toBeNull();
  });
  it("allows child actions without replacing the global title and exposes description on demand", () => {
    render(<Shell><PageChrome title="Access" description="Manage access policies" actions={<button>Create</button>} /><PageActions><button>Refresh users</button></PageActions></Shell>);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Access");
    expect(within(screen.getByRole("banner")).getByRole("button", { name: "Refresh users" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Manage access policies" }));
    expect(screen.getByText("Manage access policies")).toBeVisible();
  });
});
