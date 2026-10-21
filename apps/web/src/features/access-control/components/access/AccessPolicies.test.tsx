import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import AccessPolicies from "./AccessPolicies";
import { AllPermissions } from "@/test/permissions";
import { PermissionsContext, type PermissionsValue } from "@/shared/auth/permissionsContext";
import type { Policy } from "@/shared/api/types";
import type * as AccessDataModule from "./AccessDataContext";
import type * as AccessControlApi from "@/features/access-control/api";
import type * as InventoryApi from "@/features/inventory/api";
import type * as SecretsApi from "@/features/automation-secrets/api";

/* ── Mocks ─────────────────────────────────────────────────────────────────
   The policy list is driven by `listPolicies`; the builder's data (roles,
   users, mutation callbacks, …) comes from the AccessDataContext hooks. We
   override the two hooks but keep the real `actionCatalog` (the actions picker
   maps over it) via `importOriginal`. Everything else that would hit the
   network (secrets, id-entity lookups) is stubbed to resolve empty/known. */

const accessDataMock = vi.hoisted(() => {
  let counter = 0;
  return {
    createPolicy: vi.fn(),
    updatePolicy: vi.fn(),
    deletePolicy: vi.fn(),
    enablePolicy: vi.fn(),
    disablePolicy: vi.fn(),
    uid: () => `uid-${counter++}`,
  };
});

vi.mock("./AccessDataContext", async (importOriginal) => {
  const actual = await importOriginal<typeof AccessDataModule>();
  return {
    ...actual,
    useAccessData: () =>
      ({
        createPolicy: accessDataMock.createPolicy,
        updatePolicy: accessDataMock.updatePolicy,
        deletePolicy: accessDataMock.deletePolicy,
        enablePolicy: accessDataMock.enablePolicy,
        disablePolicy: accessDataMock.disablePolicy,
        roles: [{ id: "role-1", name: "Admins" }],
        groups: [{ id: "group-1", name: "SRE" }],
        users: [{ id: "user-1", username: "alice" }],
        servicePrincipals: [{ id: "sp-1", name: "bot" }],
        resourceTypes: ["connection_target", "secret_variable"],
        constraintAttributeKeys: { connection_target: ["protocol", "site_id"] },
        uid: accessDataMock.uid,
      }) as unknown as ReturnType<typeof actual.useAccessData>,
    useAccessLabels: () =>
      ({
        role: (value: string) => ({ id: value }),
        subject: (value: string) => ({ id: value }),
        policySubject: (rule: { id: string; label?: string }) => ({
          id: rule.id,
          name: rule.label ?? rule.id,
        }),
      }) as unknown as ReturnType<typeof actual.useAccessLabels>,
  };
});

const listPoliciesMock = vi.hoisted(() => vi.fn());
vi.mock("@/features/access-control/api", async (importOriginal) => ({
  ...(await importOriginal<typeof AccessControlApi>()),
  listPolicies: listPoliciesMock,
}));

const inventoryMock = vi.hoisted(() => ({ listAllResources: vi.fn() }));
vi.mock("@/features/inventory/api", async (importOriginal) => ({
  ...(await importOriginal<typeof InventoryApi>()),
  listAllResources: inventoryMock.listAllResources,
}));

const secretsMock = vi.hoisted(() => ({ listAutomationSecrets: vi.fn() }));
vi.mock("@/features/automation-secrets/api", async (importOriginal) => ({
  ...(await importOriginal<typeof SecretsApi>()),
  listAutomationSecrets: secretsMock.listAutomationSecrets,
}));

vi.mock("@/shared/hooks/useTablePageSize", () => ({
  useTablePageSize: () => ({ value: 25, setValue: vi.fn(), isLoading: false }),
}));

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({ notify: notifyMock }));

/* ── Fixtures ──────────────────────────────────────────────────────────── */

type PolicyOverride = Partial<Omit<Policy, "id">> & { id?: string };

const mkPolicy = (over: PolicyOverride): Policy =>
  ({
    id: "pol-x",
    name: "Policy",
    description: "",
    effect: "allow",
    enabled: true,
    actions: [],
    subjects: [],
    scopes: [],
    ...over,
  }) as Policy;

// A richly-populated allow policy that lights up every list-cell branch and,
// on edit, the whole builder dialog (subjects >3, actions >2, scopes >2, a
// protocol attribute constraint).
const fullPolicy = mkPolicy({
  id: "pol-1",
  name: "Full access rule",
  description: "Broad allow rule",
  effect: "allow",
  enabled: true,
  actions: [
    { id: "a1", action: "READ" },
    { id: "a2", action: "CONNECT" },
    { id: "a3", action: "EXECUTE" },
  ],
  subjects: [
    { id: "user-1", kind: "user", value: "user:user-1", label: "alice" },
    { id: "group-1", kind: "group", value: "group:group-1", label: "SRE" },
    { id: "role-1", kind: "role", value: "role:role-1", label: "Admins" },
    { id: "sp-1", kind: "service_principal", value: "service_principal:sp-1", label: "bot" },
  ],
  scopes: [
    { id: "s1", type: "resource_type", value: "connection_target" },
    { id: "s2", type: "resource_type", value: "secret_variable" },
    { id: "s3", type: "resource", value: "secret_variable:sec-1" },
  ],
  constraints: { attributes: { protocol: "vnc" } },
});

// A minimal disabled deny policy exercising the singular/empty column branches
// (OFF badge, deny chip, em-dash subjects, Global scope).
const minimalPolicy = mkPolicy({
  id: "pol-2",
  name: "Locked deny rule",
  description: "",
  effect: "deny",
  enabled: false,
  actions: [{ id: "b1", action: "DELETE" }],
  subjects: [],
  scopes: [],
});

const page = (items: Policy[]) => ({ items, total: items.length });

/* ── Render helpers ───────────────────────────────────────────────────── */

function renderWith(ui: ReactElement, permissions?: PermissionsValue) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const wrapped = permissions ? (
    <PermissionsContext.Provider value={permissions}>{ui}</PermissionsContext.Provider>
  ) : (
    <AllPermissions>{ui}</AllPermissions>
  );
  return render(<QueryClientProvider client={client}>{wrapped}</QueryClientProvider>);
}

const DENY_ALL: PermissionsValue = {
  roles: [],
  isAdmin: false,
  can: () => false,
  canAny: () => false,
  hasRole: () => false,
};

beforeEach(() => {
  vi.clearAllMocks();
  listPoliciesMock.mockResolvedValue(page([fullPolicy, minimalPolicy]));
  inventoryMock.listAllResources.mockResolvedValue([]);
  secretsMock.listAutomationSecrets.mockResolvedValue([
    { id: "sec-1", key: "grad-token", description: null, has_value: true, is_masked: true, usage_count: 0, created_at: "2026-07-01T00:00:00Z" },
  ]);
});

describe("AccessPolicies list", () => {
  it("renders each policy's name, effect, actions and scope cells", async () => {
    renderWith(<AccessPolicies />);

    expect(await screen.findByText("Full access rule")).toBeInTheDocument();
    expect(screen.getByText("Locked deny rule")).toBeInTheDocument();

    // Effect chips (both allow and deny variants render).
    expect(screen.getByText("Allow")).toBeInTheDocument();
    expect(screen.getByText("Deny")).toBeInTheDocument();

    // Action chips: the first two actions render, the rest collapse into "+1".
    expect(screen.getByText("READ")).toBeInTheDocument();
    expect(screen.getByText("CONNECT")).toBeInTheDocument();

    // Scope chip resolves the localized resource-type label.
    expect(screen.getByText("Connection target")).toBeInTheDocument();
  });

  it("marks a disabled policy with an OFF badge and an empty (Global) scope", async () => {
    listPoliciesMock.mockResolvedValue(page([minimalPolicy]));
    renderWith(<AccessPolicies />);

    await screen.findByText("Locked deny rule");
    expect(screen.getByText("OFF")).toBeInTheDocument();
    // No scopes → the Global placeholder renders in the scope cell.
    expect(screen.getByText("Global")).toBeInTheDocument();
  });

  it("shows the empty state with a create action when there are no policies", async () => {
    listPoliciesMock.mockResolvedValue(page([]));
    renderWith(<AccessPolicies />);

    expect(await screen.findByText("No policies")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /create policy/i })).toBeInTheDocument();
  });

  it("surfaces a load error and retries on demand", async () => {
    listPoliciesMock.mockRejectedValueOnce(new Error("backend exploded"));
    renderWith(<AccessPolicies />);

    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText("backend exploded")).toBeInTheDocument();

    // The retry control re-invokes the query (which now succeeds).
    fireEvent.click(within(alert).getByRole("button", { name: /retry/i }));
    expect(await screen.findByText("Full access rule")).toBeInTheDocument();
  });
});

describe("AccessPolicies enable/disable toggle", () => {
  it("disables an enabled policy from the row switch", async () => {
    listPoliciesMock.mockResolvedValue(page([fullPolicy]));
    accessDataMock.disablePolicy.mockResolvedValue(undefined);
    renderWith(<AccessPolicies />);
    await screen.findByText("Full access rule");

    fireEvent.click(screen.getByRole("switch"));

    await waitFor(() => expect(accessDataMock.disablePolicy).toHaveBeenCalledWith("pol-1"));
    expect(accessDataMock.enablePolicy).not.toHaveBeenCalled();
  });

  it("enables a disabled policy from the row switch", async () => {
    listPoliciesMock.mockResolvedValue(page([minimalPolicy]));
    accessDataMock.enablePolicy.mockResolvedValue(undefined);
    renderWith(<AccessPolicies />);
    await screen.findByText("Locked deny rule");

    fireEvent.click(screen.getByRole("switch"));

    await waitFor(() => expect(accessDataMock.enablePolicy).toHaveBeenCalledWith("pol-2"));
    expect(accessDataMock.disablePolicy).not.toHaveBeenCalled();
  });
});

describe("AccessPolicies create flow", () => {
  it("opens the builder dialog in create mode", async () => {
    renderWith(<AccessPolicies />);
    await screen.findByText("Full access rule");

    fireEvent.click(screen.getByRole("button", { name: /new rule/i }));

    const dialog = await screen.findByTestId("entity-editor-page");
    expect(within(dialog).getByRole("button", { name: /create policy/i })).toBeInTheDocument();
    // The name field starts empty.
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("");
    expect(within(dialog).getByLabelText("Description")).toHaveValue("");
    expect(within(dialog).getByRole("group", { name: "Effect" })).toBeInTheDocument();
  });

  it("blocks save and shows an inline error when the name is empty", async () => {
    renderWith(<AccessPolicies />);
    await screen.findByText("Full access rule");
    fireEvent.click(screen.getByRole("button", { name: /new rule/i }));
    const dialog = await screen.findByTestId("entity-editor-page");

    fireEvent.click(within(dialog).getByRole("button", { name: /create policy/i }));

    expect(within(dialog).getByText("Name is required")).toBeInTheDocument();
    expect(accessDataMock.createPolicy).not.toHaveBeenCalled();
  });

  it("blocks save and notifies when no action is selected", async () => {
    renderWith(<AccessPolicies />);
    await screen.findByText("Full access rule");
    fireEvent.click(screen.getByRole("button", { name: /new rule/i }));
    const dialog = await screen.findByTestId("entity-editor-page");

    fireEvent.change(within(dialog).getByPlaceholderText("Read-only Access"), {
      target: { value: "My new rule" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /create policy/i }));

    expect(notifyMock).toHaveBeenCalledWith("error", "Select at least one action");
    expect(accessDataMock.createPolicy).not.toHaveBeenCalled();
  });

  it("creates a policy with a name and selected actions", async () => {
    accessDataMock.createPolicy.mockResolvedValue(undefined);
    renderWith(<AccessPolicies />);
    await screen.findByText("Full access rule");
    fireEvent.click(screen.getByRole("button", { name: /new rule/i }));
    const dialog = await screen.findByTestId("entity-editor-page");

    fireEvent.change(within(dialog).getByPlaceholderText("Read-only Access"), {
      target: { value: "My new rule" },
    });
    // The toolbar "Select all" is a plain button (unlike the Radix checkboxes).
    fireEvent.click(within(dialog).getByRole("button", { name: /^select all$/i }));
    fireEvent.click(within(dialog).getByRole("button", { name: /create policy/i }));

    await waitFor(() => expect(accessDataMock.createPolicy).toHaveBeenCalledTimes(1));
    const payload = accessDataMock.createPolicy.mock.calls[0]![0];
    expect(payload).toMatchObject({ name: "My new rule", effect: "allow", enabled: true });
    expect(payload.actions.length).toBeGreaterThan(0);
    expect(payload.actions).toEqual(
      expect.arrayContaining([expect.objectContaining({ action: "READ" })]),
    );
  });
});

describe("AccessPolicies edit flow", () => {
  it("returns from the actual row menu's embedded editor to its connected trigger on Back", async () => {
    listPoliciesMock.mockResolvedValue(page([fullPolicy]));
    renderWith(<main id="main-content"><AccessPolicies /></main>);
    const trigger = await screen.findByRole("button", { name: "Actions: Full access rule" });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    const edit = await screen.findByRole("menuitem", { name: "Edit" });
    edit.focus();
    fireEvent.keyDown(edit, { key: "Enter" });
    const editor = await screen.findByTestId("entity-editor-page");
    expect(within(editor).getByRole("heading", { level: 2, name: "Edit Policy" })).toBeVisible();
    fireEvent.click(within(editor).getByRole("button", { name: "Back" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.queryByTestId("entity-editor-page")).toBeNull();
    expect(accessDataMock.updatePolicy).not.toHaveBeenCalled();
  });
  it("opens the builder pre-filled from an existing policy", async () => {
    listPoliciesMock.mockResolvedValue(page([fullPolicy]));
    renderWith(<AccessPolicies />);
    await screen.findByText("Full access rule");

    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Full access rule" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const dialog = await screen.findByTestId("entity-editor-page");

    expect(within(dialog).getByText("Edit Policy")).toBeInTheDocument();
    // Name seeded from the policy.
    expect(within(dialog).getByDisplayValue("Full access rule")).toBeInTheDocument();
    // The stored protocol attribute constraint round-trips into the preview.
    expect(within(dialog).getByText("Protocol = vnc")).toBeInTheDocument();
    // The effective-permissions preview leads with the (uppercased) effect.
    expect(within(dialog).getByText("ALLOW")).toBeInTheDocument();
    // A resolved subject label is shown (chip + preview).
    expect(within(dialog).getAllByText("alice").length).toBeGreaterThan(0);
  });

  it("saves edits through updatePolicy", async () => {
    listPoliciesMock.mockResolvedValue(page([fullPolicy]));
    accessDataMock.updatePolicy.mockResolvedValue(undefined);
    renderWith(<AccessPolicies />);
    await screen.findByText("Full access rule");
    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Full access rule" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const dialog = await screen.findByTestId("entity-editor-page");

    fireEvent.change(within(dialog).getByDisplayValue("Full access rule"), {
      target: { value: "Renamed rule" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: /save changes/i }));

    await waitFor(() => expect(accessDataMock.updatePolicy).toHaveBeenCalledTimes(1));
    const [id, payload] = accessDataMock.updatePolicy.mock.calls[0]!;
    expect(id).toBe("pol-1");
    expect(payload).toMatchObject({ name: "Renamed rule", effect: "allow" });
  });
});

describe("AccessPolicies delete flow", () => {
  it("cancels deletion without a write and returns focus to the row menu", async () => {
    listPoliciesMock.mockResolvedValue(page([minimalPolicy]));
    renderWith(<AccessPolicies />);
    const trigger = await screen.findByRole("button", { name: "Actions: Locked deny rule" });
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(accessDataMock.deletePolicy).not.toHaveBeenCalled();
  });
  it("confirms then deletes a policy", async () => {
    listPoliciesMock.mockResolvedValue(page([minimalPolicy]));
    accessDataMock.deletePolicy.mockResolvedValue(undefined);
    renderWith(<AccessPolicies />);
    await screen.findByText("Locked deny rule");

    fireEvent.keyDown(screen.getByRole("button", { name: "Actions: Locked deny rule" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));

    const confirm = await screen.findByRole("alertdialog");
    expect(within(confirm).getByText("Delete Policy")).toBeInTheDocument();
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete" }));

    await waitFor(() => expect(accessDataMock.deletePolicy).toHaveBeenCalledWith("pol-2"));
  });
});

describe("AccessPolicies permission gating", () => {
  it("keeps delete-only controls independent from update controls", async () => {
    listPoliciesMock.mockResolvedValue(page([fullPolicy]));
    renderWith(<AccessPolicies />, { ...DENY_ALL, can: action => action === "READ" || action === "DELETE" });
    expect(await screen.findByRole("switch")).toBeDisabled();
    fireEvent.keyDown(await screen.findByRole("button", { name: "Actions: Full access rule" }), { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: "Delete" })).toBeEnabled();
    expect(screen.queryByRole("menuitem", { name: "Edit" })).toBeNull();
    expect(screen.queryByRole("button", { name: /new rule/i })).toBeNull();
  });
  it("hides mutation affordances for a read-only viewer", async () => {
    listPoliciesMock.mockResolvedValue(page([fullPolicy]));
    renderWith(<AccessPolicies />, DENY_ALL);
    await screen.findByText("Full access rule");

    expect(screen.queryByRole("button", { name: /new rule/i })).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Actions:/ })).toBeNull();
    // The enabled switch is still shown but not operable.
    expect(screen.getByRole("switch")).toBeDisabled();
  });
});
