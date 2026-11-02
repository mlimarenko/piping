import { fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import "@/shared/i18n";
import { AllPermissions } from "@/test/permissions";

import PlaybookEditorPage from "./PlaybookEditorPage";
import type { PlaybookCatalogItem } from "./api";

const playbookApiMock = vi.hoisted(() => ({
  getPlaybook: vi.fn(),
  listPlaybookTargetSets: vi.fn(),
}));

vi.mock("./api", () => ({
  getPlaybook: playbookApiMock.getPlaybook,
  listPlaybookTargetSets: playbookApiMock.listPlaybookTargetSets,
}));

const secretsApiMock = vi.hoisted(() => ({
  listAutomationSecrets: vi.fn(),
}));

vi.mock("@/features/automation-secrets/api", () => ({
  AUTOMATION_SECRETS_QUERY_KEY: ["automation", "secrets"],
  listAutomationSecrets: secretsApiMock.listAutomationSecrets,
}));

vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));

vi.mock("./CodeEditor", () => ({
  default: ({
    value,
    onChange,
    "aria-label": ariaLabel,
  }: {
    value: string;
    onChange: (value: string) => void;
    "aria-label"?: string;
  }) => (
    <textarea
      aria-label={ariaLabel ?? "code"}
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

function LocationProbe() {
  const location = useLocation();
  return <span data-testid="location-path">{location.pathname}{location.search}</span>;
}

const item = {
  playbook: {
    id: "playbook-1",
    name: "Linux inventory",
    slug: "linux-inventory",
    description: "Collect inventory",
    category: "inventory",
    runtime: "ansible",
    connection_default: "ssh",
    risk: "low",
    status: "active",
    source_kind: "in_app",
    default_target_set_id: null,
    current_version_id: "version-1",
    vars_schema: {},
    metadata: {},
    created_at: "2026-06-06T00:00:00Z",
    updated_at: "2026-06-06T00:00:00Z",
  },
  current_version: {
    id: "version-1",
    version: 1,
    content: "---\n- hosts: all\n",
    timeout_seconds: 900,
  },
} as PlaybookCatalogItem;

function renderPage(initialEntry: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <MemoryRouter initialEntries={[initialEntry]}>
          <Routes>
            <Route path="/playbooks/new" element={<PlaybookEditorPage />} />
            <Route path="/playbooks/:id/edit" element={<PlaybookEditorPage />} />
            <Route path="/playbooks/:id" element={<p>Parent context</p>} />
            <Route path="/playbooks" element={<p>Catalog list</p>} />
          </Routes>
          <LocationProbe />
        </MemoryRouter>
      </AllPermissions>
    </QueryClientProvider>,
  );
}

describe("PlaybookEditorPage (routed)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    playbookApiMock.listPlaybookTargetSets.mockResolvedValue({ items: [] });
    secretsApiMock.listAutomationSecrets.mockResolvedValue([]);
  });

  it("deep-links into the edit editor and restores the entity from the URL", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(item);

    renderPage("/playbooks/playbook-1/edit");

    expect(await screen.findByRole("heading", { name: /Linux inventory/ })).toBeInTheDocument();
    expect(playbookApiMock.getPlaybook).toHaveBeenCalledWith("playbook-1");
    // The name field is pre-filled from the fetched entity (edit reload).
    expect(await screen.findByDisplayValue("Linux inventory")).toBeInTheDocument();
  });

  it("returns to the catalog list on back", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(item);

    renderPage("/playbooks/playbook-1/edit");
    await screen.findByRole("heading", { name: /Linux inventory/ });

    fireEvent.click(screen.getByRole("button", { name: /^back$/i }));

    await waitFor(() => {
      expect(screen.getByTestId("location-path")).toHaveTextContent("/playbooks");
    });
  });

  it("returns to the selected parent section and filters after editor reload", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(item);
    const returnTo = "/playbooks/playbook-1?section=versions&assignments.enabled=false";
    renderPage(`/playbooks/playbook-1/edit?${new URLSearchParams({ returnTo })}`);
    await screen.findByRole("heading", { name: /Linux inventory/ });
    fireEvent.click(screen.getByRole("button", { name: /^back$/i }));
    expect(await screen.findByText("Parent context")).toBeInTheDocument();
    expect(screen.getByTestId("location-path").textContent).toBe(returnTo);
  });

  it("shows the unsaved-changes guard once the form is edited", async () => {
    playbookApiMock.getPlaybook.mockResolvedValue(item);

    renderPage("/playbooks/playbook-1/edit");
    await screen.findByRole("heading", { name: /Linux inventory/ });

    expect(screen.queryByText(/unsaved changes/i)).not.toBeInTheDocument();

    fireEvent.change(screen.getByDisplayValue("Linux inventory"), {
      target: { value: "Linux inventory edited" },
    });

    expect(await screen.findByText(/unsaved changes/i)).toBeInTheDocument();
  });
});
