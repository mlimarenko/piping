import { fireEvent, screen, within } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";
import CatalogDetailSheet, { type CatalogDetailNames, type CatalogSelection } from "./CatalogDetailSheet";
import type { AddressGroup, Application, DataflowGroup } from "./dataflowCatalog-api";

const flowsApi = vi.hoisted(() => ({ resolveDataflowEndpoint: vi.fn() }));
vi.mock("./dataflows-api", () => flowsApi);

const application = {
  id: "app-1", name: "Payments", role_id: "role-1", description: "Card payments", comments: "Renew yearly",
  owner_id: "owner-1", tenant_id: "tenant-1", tags: ["tag-1"], custom_field_data: {},
} satisfies Application;
const group = {
  id: "group-edge", parent_id: "group-core", application_id: "app-1", name: "Edge", status: "active", effective_status: "disabled",
  blocked_by_group_id: "group-core", tags: ["tag-2"], inherited_tags: ["tag-1"], effective_tags: ["tag-1", "tag-2"],
  description: "", comments: "Paused by the platform owner", custom_field_data: {},
} satisfies DataflowGroup;
const addressGroup = {
  id: "ag-office", name: "Office", description: "HQ ranges", comments: "", owner_id: "owner-1", tenant_id: "tenant-1", tags: ["tag-1"],
  members: [{ object_type: "dcim.device", object_id: "device-1", address_mode: "default", ipaddresses: [], position: 0 }], custom_field_data: {},
} satisfies AddressGroup;

const names: CatalogDetailNames = {
  role: () => "Core",
  application: () => "Payments",
  group: id => id === "group-core" ? "Core" : null,
  owner: () => "Alice",
  tenant: () => "Acme",
  tags: ids => (ids ?? []).map(id => id === "tag-1" ? "prod" : "campus"),
};

function mount(selection: CatalogSelection, props: Partial<Parameters<typeof CatalogDetailSheet>[0]> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><MemoryRouter>
    <CatalogDetailSheet selection={selection} names={names} onClose={() => {}} onCloseAutoFocus={() => {}} {...props} />
  </MemoryRouter></QueryClientProvider>);
}

describe("catalog detail sheet", () => {
  it("reads the whole application record with a scoped flows link", async () => {
    mount({ kind: "application", item: application });
    const dialog = await screen.findByRole("dialog", { name: "Payments" });
    expect(within(dialog).getByText("Core")).toBeInTheDocument();
    expect(within(dialog).getByText("Card payments")).toBeInTheDocument();
    expect(within(dialog).getByText("Renew yearly")).toBeInTheDocument();
    expect(within(dialog).getByText("Alice")).toBeInTheDocument();
    expect(within(dialog).getByText("Acme")).toBeInTheDocument();
    expect(within(dialog).getByText("prod")).toBeInTheDocument();
    expect(within(dialog).getByRole("link", { name: "Flows" })).toHaveAttribute("href", "/inventory/dataflows?application=app-1");
    const contacts = new URL(within(dialog).getByRole("link", { name: "Application contacts" }).getAttribute("href")!, "https://piping.invalid");
    expect(contacts.searchParams.get("content_type")).toBe("inventory.application");
    expect(contacts.searchParams.get("object_id")).toBe(application.id);
    expect(within(dialog).queryByRole("link", { name: "Edit" })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
  });

  it("separates direct, inherited and effective group tags with hierarchy context", async () => {
    mount({ kind: "group", item: group });
    const dialog = await screen.findByRole("dialog", { name: "Edge" });
    expect(within(dialog).getAllByText("Core").length).toBeGreaterThan(0);
    expect(within(dialog).getByText("Payments")).toBeInTheDocument();
    expect(within(dialog).getByText("Active")).toBeInTheDocument();
    expect(within(dialog).getByText("Disabled")).toBeInTheDocument();
    expect(within(dialog).getByText("Blocked by")).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Direct tags" })).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Inherited tags" })).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Effective tags" })).toBeInTheDocument();
    expect(within(dialog).getAllByText("campus").length).toBe(2);
    expect(within(dialog).getAllByText("prod").length).toBe(2);
    expect(within(dialog).getByRole("link", { name: "Flows" })).toHaveAttribute("href", "/inventory/dataflows?group=group-edge");
  });

  it("previews address group members through the resolver", async () => {
    flowsApi.resolveDataflowEndpoint.mockResolvedValue({
      endpoint: { object_type: "dcim.device", object_id: "device-1", address_mode: "default", ipaddresses: [] },
      addresses: [{ cidr: "10.0.0.10/32", vrf_id: null }],
      default_ipaddresses: ["ip-1"],
      diagnostics: [],
      ipaddresses: [],
      members: [{ host: { object_type: "dcim.device", object_id: "device-1", label: "Web host" }, addresses: [{ cidr: "10.0.0.10/32", vrf_id: null }], diagnostics: [] }],
    });
    mount({ kind: "addressGroup", item: addressGroup });
    const dialog = await screen.findByRole("dialog", { name: "Office" });
    expect(within(dialog).getByText("Objects in this group: 1")).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Object 1" })).toBeInTheDocument();
    expect(await within(dialog).findByText(/10\.0\.0\.10\/32/)).toBeInTheDocument();
    expect(within(dialog).getByRole("link", { name: "Flows" }))
      .toHaveAttribute("href", "/inventory/dataflows?endpointType=inventory.addressgroup&endpointId=ag-office");
  });

  it("keeps edit and delete behind their own flags and reports a blocked delete inline", async () => {
    mount({ kind: "application", item: application }, {
      canEdit: true, editPath: "/inventory/dataflows/applications/app-1/edit",
      canDelete: true, onDelete: () => {}, deleteErrorMessage: "Delete is blocked by related records",
    });
    const dialog = await screen.findByRole("dialog", { name: "Payments" });
    expect(within(dialog).getByRole("link", { name: "Edit" })).toHaveAttribute("href", "/inventory/dataflows/applications/app-1/edit");
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Delete is blocked by related records");
    expect(within(dialog).getByRole("group", { name: "Delete" })).toBeInTheDocument();
  });

});
