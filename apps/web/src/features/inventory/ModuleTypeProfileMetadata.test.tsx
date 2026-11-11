import { useState } from "react";
import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "@/test/pageChrome";
import { AllPermissions } from "@/test/permissions";
import i18n from "@/shared/i18n";
import EntityFormBody from "@/shared/forms/EntityFormBody";
import { buildEntityDraft, draftToResourceUpdate } from "@/shared/forms/entityForm";
import { getResource, updateResource } from "./api";

const api = vi.hoisted(() => ({ GET: vi.fn(), PATCH: vi.fn() }));
vi.mock("@/shared/api/client", () => ({ apiClient: api, unwrap: (result: { data?: unknown }) => result.data }));
const tagId = "00000000-0000-4000-8000-000000000080";
beforeEach(async () => { vi.clearAllMocks(); await i18n.changeLanguage("en"); });

describe("Module type profile native metadata", () => {
  it("keeps unknown historical dates unknown rather than using synchronization time", async () => {
    api.GET.mockResolvedValue({ data: { id: "p", name: "Imported profile", tags: [tagId], created: null, last_updated: null, piping_synced_at: "2026-10-27T01:00:00Z" } });
    expect(await getResource("Module Type Profiles", "p")).toMatchObject({ tags: [tagId], createdAt: undefined, updatedAt: undefined });
    api.GET.mockResolvedValue({ data: { id: "p", name: "Native profile", tags: [], created: "2026-10-27T02:00:00Z", last_updated: "2026-10-27T03:00:00Z" } });
    expect(await getResource("Module Type Profiles", "p")).toMatchObject({ createdAt: "2026-10-27T02:00:00Z", updatedAt: "2026-10-27T03:00:00Z" });
  });

  it("sends canonical Tag UUID memberships and distinguishes omission from clearing", async () => {
    api.PATCH.mockResolvedValue({ data: { id: "p", name: "Profile" } });
    await updateResource("Module Type Profiles", "p", { tags: [tagId] });
    expect(api.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/module-type-profiles/{id}", { params: { path: { id: "p" } }, body: { tags: [tagId] } });
    await updateResource("Module Type Profiles", "p", { comments: "keep tags" });
    expect(api.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/module-type-profiles/{id}", { params: { path: { id: "p" } }, body: { comments: "keep tags" } });
    await updateResource("Module Type Profiles", "p", { tags: [] });
    expect(api.PATCH).toHaveBeenLastCalledWith("/api/v1/inventory/module-type-profiles/{id}", { params: { path: { id: "p" } }, body: { tags: [] } });
  });

  it("uses the existing shared Tag picker and emits a changed-only empty set", () => {
    function Form() {
      const resource = { id: "p", name: "Profile", type: "module_type_profile", tags: [tagId] };
      const initial = buildEntityDraft("Module Type Profiles", resource);
      const [draft, setDraft] = useState(initial);
      return <><EntityFormBody entity="Module Type Profiles" draft={draft} onField={(key, value) => setDraft(previous => ({ ...previous, [key]: value }))}
        loadOptions={entity => entity === "Tags" ? [{ id: tagId, name: "Core", objectTypes: [] }] : []} onRequestCreate={() => undefined} />
        <output data-testid="profile-metadata-payload">{JSON.stringify(draftToResourceUpdate("Module Type Profiles", draft, "Module Type Profile", resource, initial))}</output></>;
    }
    render(<AllPermissions><Form /></AllPermissions>);
    expect(screen.getByTestId("profile-metadata-payload")).toHaveTextContent('{"id":"p"}');
    fireEvent.click(screen.getByRole("button", { name: "Remove Core" }));
    expect(screen.getByTestId("profile-metadata-payload")).toHaveTextContent('{"id":"p","tags":[]}');
  });
});
