import { useEffect, useState } from "react";
import { MemoryRouter, useNavigate, useLocation } from "react-router";
import { fireEvent, screen, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext } from "@/shared/auth/permissionsContext";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ConnectionsWorkspaceOutlet from "./ConnectionsWorkspaceOutlet";

const { mounts, unmounts } = vi.hoisted(() => ({ mounts: vi.fn(), unmounts: vi.fn() }));
vi.mock("./ConnectionsWorkspace", () => ({ default: function Workspace({ visible }: { visible: boolean }) {
  useEffect(() => { mounts(); return () => unmounts(); }, []);
  return <><PageChrome title="Connections" visible={visible} actions={<button>Connection action</button>} /><input aria-label="Session buffer" data-active={String(visible)} defaultValue="output" /></>;
} }));

function Navigation() {
  const navigate = useNavigate();
  return <><button onClick={() => void navigate("/inventory/resources/Devices/device/edit")}>Edit device</button>
    <button onClick={() => void navigate("/connections")}>Return</button><ConnectionsWorkspaceOutlet /></>;
}

describe("authenticated connections workspace lifetime", () => {
  beforeEach(() => { mounts.mockClear(); unmounts.mockClear(); });
  it("does not mount a viewer before the first visit and retains it on later visits", async () => {
    render(<AllPermissions><MemoryRouter initialEntries={["/inventory/resources/Devices/device/edit"]}><Navigation /></MemoryRouter></AllPermissions>);
    expect(screen.queryByRole("textbox", { name: "Session buffer" })).not.toBeInTheDocument();
    expect(mounts).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Return" }));
    const viewer = await screen.findByRole("textbox", { name: "Session buffer" });
    fireEvent.click(screen.getByRole("button", { name: "Edit device" }));
    expect(viewer.closest("[hidden][inert]")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Return" }));
    expect(screen.getByRole("textbox", { name: "Session buffer" })).toBe(viewer);
    expect(mounts).toHaveBeenCalledTimes(1);
    expect(unmounts).not.toHaveBeenCalled();
  });
  it("retains the same viewer buffer while editing and disables off-route input", async () => {
    render(<AllPermissions><MemoryRouter initialEntries={["/connections"]}><Navigation /></MemoryRouter></AllPermissions>);
    const viewer = await screen.findByRole("textbox", { name: "Session buffer" });
    fireEvent.change(viewer, { target: { value: "unsent terminal buffer" } });
    fireEvent.click(screen.getByRole("button", { name: "Edit device" }));
    expect(viewer).toHaveAttribute("data-active", "false");
    expect(viewer.closest("[inert]")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Return" }));
    expect(screen.getByRole("textbox", { name: "Session buffer" })).toBe(viewer);
    expect(viewer).toHaveValue("unsent terminal buffer");
    expect(mounts).toHaveBeenCalledTimes(1);
    expect(unmounts).not.toHaveBeenCalled();
  });
  it("does not let a hidden retained denial steal the active inventory chrome on permission revocation", async () => {
    function Fixture() {
      const [allowed, setAllowed] = useState(true);
      const navigate = useNavigate();
      const visible = useLocation().pathname === "/connections";
      return <PermissionsContext.Provider value={{ roles: [], isAdmin: false, can: (action, resource) => action !== "READ" || resource !== "CONNECTION_TARGET" || allowed, canAny: () => true, hasRole: () => false }}>
        <button onClick={() => void navigate("/inventory/resources")}>Inventory</button><button onClick={() => void navigate("/connections")}>Connections</button>
        <button onClick={() => setAllowed(false)}>Revoke connections</button><ConnectionsWorkspaceOutlet />
        {!visible && <PageChrome title="Inventory" actions={<button>Create inventory</button>} />}
      </PermissionsContext.Provider>;
    }
    render(<MemoryRouter initialEntries={["/connections"]}><Fixture /></MemoryRouter>);
    await screen.findByRole("textbox", { name: "Session buffer" });
    fireEvent.click(screen.getByRole("button", { name: "Inventory" }));
    const header = screen.getByRole("banner");
    expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Inventory");
    fireEvent.click(screen.getByRole("button", { name: "Revoke connections" }));
    expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("Inventory");
    expect(within(header).getByRole("button", { name: "Create inventory" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Connections" }));
    expect(within(header).getByRole("heading", { level: 1 })).toHaveTextContent("You don't have access to this section");
    expect(screen.queryByRole("button", { name: "Create inventory" })).toBeNull();
  });
});
