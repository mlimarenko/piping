import { fireEvent, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ComponentProps } from "react";

import { AllPermissions } from "@/test/permissions";
import { render } from "@/test/pageChrome";
import type { TargetSetEditor } from "./TargetSetEditor";
import TargetSetEditorPage from "./TargetSetEditorPage";
import { TARGET_SET_EDITOR_EDIT_PATH } from "./playbookEditorRoutes";
import "@/shared/i18n";

const api = vi.hoisted(() => ({
  get: vi.fn(),
  update: vi.fn(),
  create: vi.fn(),
  connections: vi.fn(),
  preview: vi.fn(),
}));

vi.mock("./api", () => ({
  getTargetSet: api.get,
  updateTargetSet: api.update,
  createPlaybookTargetSet: api.create,
  previewTargetSelector: api.preview,
}));
vi.mock("@/features/connections/api", () => ({
  getConnectionTarget: api.connections,
}));
vi.mock("./TargetSetEditor", () => ({
  TargetSetEditor: (props: ComponentProps<typeof TargetSetEditor>) => (
    <div>
      <output data-testid="mode">{props.mode}</output>
      <output data-testid="hosts">{props.staticHosts.length}</output>
      <button onClick={() => props.onModeChange("static")}>make static</button>
      <button onClick={() => props.onModeChange("dynamic")}>make dynamic</button>
      <button onClick={props.onSubmit}>save target</button>
      <button onClick={() => props.onSelectorModelChange({ ...props.selectorModel, facets: { ...props.selectorModel.facets, status: ["offline"] } })}>change conditions</button>
      {props.mode === "dynamic" && props.preview}
    </div>
  ),
}));

const targetSetId = "33333333-3333-4333-8333-333333333333";
const target = {
  id: "11111111-1111-1111-1111-111111111111",
  name: "web-1",
  targetKind: "device" as const,
  deviceId: "22222222-2222-2222-2222-222222222222",
  role: "web",
  site: "eu-west",
  primaryIp: "10.0.0.10",
};
const member = {
  target_kind: "connection_target",
  target_id: target.id,
  source_kind: "device",
  source_id: target.deviceId,
  sort_order: 0,
};

function renderEditor() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <AllPermissions>
        <MemoryRouter initialEntries={[`/playbooks/settings/target-sets/${targetSetId}/edit`]}>
          <Routes>
            <Route path={TARGET_SET_EDITOR_EDIT_PATH} element={<TargetSetEditorPage />} />
            <Route path="/playbooks/settings/target-sets" element={<output>list</output>} />
          </Routes>
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  );
}

describe("target-set static members", () => {
  beforeEach(() => {
    api.get.mockReset();
    api.update.mockReset();
    api.create.mockReset();
    api.connections.mockReset();
    api.connections.mockResolvedValue(target);
    api.update.mockResolvedValue({});
    api.preview.mockReset();
    api.preview.mockResolvedValue({ items: [], total: 1, limit: 50, truncated: false });
  });

  it("updates the unsaved preview and requires explicit confirmation for an empty set", async () => {
    api.get.mockResolvedValue({ id: targetSetId, name: "Fleet", mode: "dynamic", selector: {}, members: [] });
    api.preview.mockResolvedValue({ items: [], total: 0, limit: 50, truncated: false });
    renderEditor();
    await screen.findByText("No hosts match these conditions.");
    fireEvent.click(screen.getByRole("button", { name: "save target" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Save a set with no matching hosts?");
    expect(api.update).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.update).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "change conditions" }));
    await waitFor(() => expect(api.preview).toHaveBeenLastCalledWith(expect.objectContaining({ status: ["offline"] }), expect.any(AbortSignal)));
    await screen.findByText("No hosts match these conditions.");
    fireEvent.click(screen.getByRole("button", { name: "save target" }));
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.update).toHaveBeenCalledWith(targetSetId, expect.objectContaining({ selector: expect.objectContaining({ status: ["offline"] }) })));
  });

  it("does not turn a failed preview into an empty-set save", async () => {
    api.get.mockResolvedValue({ id: targetSetId, name: "Fleet", mode: "dynamic", selector: {}, members: [] });
    api.preview.mockRejectedValue(new Error("preview unavailable"));
    renderEditor();
    await screen.findByRole("alert");
    fireEvent.click(screen.getByRole("button", { name: "save target" }));
    expect(api.update).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it.each([false, true])("seeds persisted members and updates them when label lookup fails: %s", async (labelsUnavailable) => {
    if (labelsUnavailable) api.connections.mockRejectedValue(new Error("names unavailable"));
    api.get.mockResolvedValue({
      id: targetSetId,
      name: "Static fleet",
      mode: "static",
      entity_kind: "connection_target",
      selector: {},
      members: [member],
      created_at: "",
      updated_at: "",
    });

    renderEditor();
    await waitFor(() => expect(screen.getByTestId("hosts")).toHaveTextContent("1"));
    fireEvent.click(screen.getByRole("button", { name: "save target" }));

    await waitFor(() =>
      expect(api.update).toHaveBeenCalledWith(
        targetSetId,
        expect.objectContaining({
          mode: "static",
          members: [
            {
              target_kind: "connection_target",
              target_id: target.id,
              source_kind: "device",
              source_id: target.deviceId,
              sort_order: 0,
            },
          ],
        }),
      ),
    );
  });

  it("blocks saving a static set with no members", async () => {
    api.get.mockResolvedValue({
      id: targetSetId,
      name: "Dynamic fleet",
      mode: "dynamic",
      entity_kind: "connection_target",
      selector: { status: ["active"] },
      members: [],
      created_at: "",
      updated_at: "",
    });

    renderEditor();
    await waitFor(() => expect(screen.getByTestId("mode")).toHaveTextContent("dynamic"));
    fireEvent.click(screen.getByRole("button", { name: "make static" }));
    fireEvent.click(screen.getByRole("button", { name: "save target" }));

    expect(api.update).not.toHaveBeenCalled();
  });

  it("keeps hidden static members when the set switches to dynamic", async () => {
    api.get.mockResolvedValue({
      id: targetSetId,
      name: "Static fleet",
      mode: "static",
      entity_kind: "connection_target",
      selector: {},
      members: [member],
      created_at: "",
      updated_at: "",
    });

    renderEditor();
    await waitFor(() => expect(screen.getByTestId("hosts")).toHaveTextContent("1"));
    fireEvent.click(screen.getByRole("button", { name: "make dynamic" }));
    await screen.findByText("1 host");
    fireEvent.click(screen.getByRole("button", { name: "save target" }));

    await waitFor(() =>
      expect(api.update).toHaveBeenCalledWith(
        targetSetId,
        expect.objectContaining({
          mode: "dynamic",
          members: [expect.objectContaining({ target_id: target.id })],
        }),
      ),
    );
  });
});
