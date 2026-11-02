import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import { AccessDataProvider, useAccessData } from "./AccessDataContext";
import type * as AccessApi from "@/features/access-control/api";
import type * as InventoryApi from "@/features/inventory/api";
import { notify } from "@/shared/lib/notifications";

/* The CRUD orchestration (defineCrudMutation) lives inside AccessDataProvider,
   which no existing test exercises directly: the component tests mock
   `useAccessData`. This test drives the real provider so the shared mutation
   wiring — call the API, invalidate the owning collection, toast success — is
   pinned to behaviour rather than to source text. */

const apiMock = vi.hoisted(() => ({
  createGroup: vi.fn(),
  listAllUsers: vi.fn(),
  listAllGroups: vi.fn(),
  listAllRoles: vi.fn(),
  listPolicies: vi.fn(),
  listGrants: vi.fn(),
  listServicePrincipals: vi.fn(),
  getAccessVocabulary: vi.fn(),
}));

vi.mock("@/features/access-control/api", async (importOriginal) => ({
  ...(await importOriginal<typeof AccessApi>()),
  ...apiMock,
}));

vi.mock("@/features/inventory/api", async (importOriginal) => ({
  ...(await importOriginal<typeof InventoryApi>()),
  listResources: vi.fn().mockResolvedValue({ items: [], total: 0 }),
}));

vi.mock("@/shared/lib/notifications", () => ({
  notify: vi.fn(),
}));

function Probe() {
  const { createGroup } = useAccessData();
  return <button onClick={() => createGroup({ name: "SRE", externalId: "" })}>add</button>;
}

function renderProvider() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <AccessDataProvider>
        <Probe />
      </AccessDataProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  apiMock.listAllUsers.mockResolvedValue([]);
  apiMock.listAllGroups.mockResolvedValue([]);
  apiMock.listAllRoles.mockResolvedValue([]);
  apiMock.listPolicies.mockResolvedValue({ items: [], total: 0 });
  apiMock.listGrants.mockResolvedValue({ items: [], total: 0 });
  apiMock.listServicePrincipals.mockResolvedValue([]);
  apiMock.getAccessVocabulary.mockResolvedValue({ resources: [], actions: [] });
  apiMock.createGroup.mockResolvedValue({ id: "g1", name: "SRE", externalId: "" });
});

describe("AccessDataProvider CRUD orchestration", () => {
  it("calls the API, invalidates the owning collection, and toasts success on create", async () => {
    renderProvider();
    screen.getByRole("button", { name: "add" }).click();

    await waitFor(() => expect(apiMock.createGroup).toHaveBeenCalledWith({ name: "SRE", externalId: "" }));

    // Success toast uses the per-resource message key (resolved through i18n).
    await waitFor(() => expect(vi.mocked(notify)).toHaveBeenCalledWith("success", expect.stringContaining("created")));

    // Invalidation re-fetches the full groups collection (initial load + refresh).
    await waitFor(() => expect(apiMock.listAllGroups).toHaveBeenCalledTimes(2));
  });
});
