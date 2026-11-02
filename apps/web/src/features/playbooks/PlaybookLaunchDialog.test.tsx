import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import PlaybookLaunchDialog, { type PlaybookLaunchIntent } from "./PlaybookLaunchDialog";
import type { PlaybookCatalogItem, PlaybookTargetSetPreview } from "./api";
import type * as PlaybookApiTypes from "./api";
import type * as SecretsApiTypes from "@/features/automation-secrets/api";

const playbookApiMock = vi.hoisted(() => ({
  previewPlaybookTargetSet: vi.fn(),
}));

const secretsApiMock = vi.hoisted(() => ({
  playbookSecretsPreflight: vi.fn(),
}));

vi.mock("@/features/playbooks/api", async (importOriginal) => ({
  ...(await importOriginal<typeof PlaybookApiTypes>()),
  previewPlaybookTargetSet: playbookApiMock.previewPlaybookTargetSet,
}));

vi.mock("@/features/automation-secrets/api", async (importOriginal) => ({
  ...(await importOriginal<typeof SecretsApiTypes>()),
  playbookSecretsPreflight: secretsApiMock.playbookSecretsPreflight,
}));

const playbook: PlaybookCatalogItem = {
  playbook: {
    id: "playbook-1",
    name: "Linux inventory",
    slug: "linux-inventory",
    description: "Collect inventory",
    category: "inventory",
    runtime: "ansible",
    connection_default: "ssh",
    vars_schema: {
      type: "object",
      properties: {
        package: {
          type: "string",
          title: "Package",
          description: "Package name passed to the Ansible playbook.",
          default: "htop",
        },
      },
      required: ["package"],
    },
    risk: "low",
    status: "active",
    default_target_set_id: null,
    current_version_id: "version-1",
    source_kind: "in_app",
    inventory_rest_enabled: false,
    metadata: {},
    created_at: "2026-06-06T00:00:00Z",
    updated_at: "2026-06-06T00:00:00Z",
  },
  current_version: {
    id: "version-1",
    playbook_id: "playbook-1",
    version: 1,
    content: "---\n- hosts: all\n  tasks: []\n",
    content_sha256: "sha256",
    runtime: "ansible",
    language: "yaml",
    timeout_seconds: 900,
    input_schema: {},
    output_schema: {},
    token_policy: {},
    metadata: {},
    created_at: "2026-06-06T00:00:00Z",
  },
};

function renderDialog(intent: PlaybookLaunchIntent, onConfirm = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  const view = render(
    <QueryClientProvider client={client}>
      <PlaybookLaunchDialog
        open
        intent={intent}
        onOpenChange={() => {}}
        onConfirm={onConfirm}
      />
    </QueryClientProvider>,
  );
  const rerenderDialog = (open: boolean, nextIntent: PlaybookLaunchIntent | null = intent) => {
    view.rerender(
      <QueryClientProvider client={client}>
        <PlaybookLaunchDialog
          open={open}
          intent={nextIntent}
          onOpenChange={() => {}}
          onConfirm={onConfirm}
        />
      </QueryClientProvider>,
    );
  };
  return { onConfirm, rerenderDialog };
}

function staticIntent(): PlaybookLaunchIntent {
  return {
    playbook,
    triggerKind: "manual",
    targetSetName: "Selected targets",
    staticTargets: [
      {
        id: "target-1",
        name: "pxm-piping01",
        subtitle: "aeza-de · Linux · 10.0.1.10",
      },
    ],
  };
}

function serverIntent(): PlaybookLaunchIntent {
  return {
    playbook,
    triggerKind: "manual",
    targetSetId: "target-set-600",
    targetSetName: "Dynamic inventory",
  };
}

function preview(total: number): PlaybookTargetSetPreview {
  return {
    target_set: {
      id: "target-set-600",
      name: "Dynamic inventory",
      mode: "dynamic",
      entity_kind: "connection_target",
      selector: {},
      created_at: "2026-06-06T00:00:00Z",
      updated_at: "2026-06-06T00:00:00Z",
    },
    items: total > 0
      ? [
          {
            target_id: "target-1",
            target_kind: "connection_target",
            source_id: "source-1",
            source_kind: "virtual_machine",
            name: "pxm-piping01",
            address_bound: true,
            address: "10.0.1.10",
          },
          {
            target_id: "target-2",
            target_kind: "connection_target",
            source_id: "source-2",
            source_kind: "device",
            name: "pxm-piping02",
            address_bound: true,
            address: "10.0.1.11",
          },
        ]
      : [],
    total,
    limit: 100,
    truncated: total > 2,
  };
}

describe("PlaybookLaunchDialog", () => {
  beforeEach(() => {
    playbookApiMock.previewPlaybookTargetSet.mockReset();
    secretsApiMock.playbookSecretsPreflight.mockReset();
    // Default: no declared secrets, so the preflight panel is inert.
    secretsApiMock.playbookSecretsPreflight.mockResolvedValue({
      all_allowed: true,
      secrets: [],
    });
  });

  it("confirms static selected targets without resolving a server target-set preview", () => {
    const { onConfirm } = renderDialog(staticIntent());

    expect(screen.getByText("pxm-piping01")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Queue run/i }));

    expect(playbookApiMock.previewPlaybookTargetSet).not.toHaveBeenCalled();
    expect(onConfirm).toHaveBeenCalledWith(
      expect.objectContaining({ playbook }),
      null,
    );
  });

  it("packs typed vars_schema defaults and connection into launch_options.extravars", () => {
    const { onConfirm } = renderDialog(staticIntent());

    expect(
      screen.getByText("Package name passed to the Ansible playbook."),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Queue run/i }));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    const submitted = onConfirm.mock.calls[0]?.[0] as PlaybookLaunchIntent;
    const launchOptions = (submitted.metadata as Record<string, unknown>)
      .launch_options as Record<string, unknown>;
    expect(launchOptions.extravars).toEqual({ package: "htop" });
    expect(launchOptions.connection).toBe("ssh");
    expect(launchOptions.target_limit).toBe(1);
  });

  it("does not block server target sets above the removed 500-target UI limit", async () => {
    playbookApiMock.previewPlaybookTargetSet.mockResolvedValueOnce(preview(600));
    const { onConfirm } = renderDialog(serverIntent());

    expect(await screen.findByText("pxm-piping01")).toBeInTheDocument();
    await waitFor(() => {
      expect(playbookApiMock.previewPlaybookTargetSet).toHaveBeenCalledWith(
        "target-set-600",
        100,
      );
    });

    const queueButton = screen.getByRole("button", { name: /Queue run \(600\)/i });
    await waitFor(() => expect(queueButton).not.toBeDisabled());
    fireEvent.click(queueButton);

    expect(onConfirm).toHaveBeenCalledTimes(1);
    const submitted = onConfirm.mock.calls[0]?.[0] as PlaybookLaunchIntent;
    const launchOptions = (submitted.metadata as Record<string, unknown>)
      .launch_options as Record<string, unknown>;
    expect(submitted.targetLimit).toBe(600);
    expect(launchOptions.target_limit).toBe(600);
  });

  it("blocks empty target sets before queueing", async () => {
    playbookApiMock.previewPlaybookTargetSet.mockResolvedValueOnce(preview(0));
    const { onConfirm } = renderDialog(serverIntent());

    expect(await screen.findByText("No eligible targets")).toBeInTheDocument();
    expect(
      screen.getByText(/The saved launch source currently resolves to zero eligible hosts/i),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Queue run \(0\)/i })).toBeDisabled();
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("blocks the launch when a declared secret preflight fails", async () => {
    secretsApiMock.playbookSecretsPreflight.mockResolvedValueOnce({
      all_allowed: false,
      secrets: [
        { env_name: "GRAD_TOKEN", key: "grad-token", secret_id: "s1", has_value: true, allowed: true },
        { env_name: "EXT_KEY", key: "ext", secret_id: "s2", has_value: true, allowed: false },
      ],
    });
    const { onConfirm } = renderDialog(staticIntent());

    const queueButton = await screen.findByRole("button", { name: /Queue run/i });
    await waitFor(() => expect(queueButton).toBeDisabled());
    expect(screen.getByText("EXT_KEY")).toBeInTheDocument();

    fireEvent.click(queueButton);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("keeps technical launch controls out of the primary review", () => {
    renderDialog(staticIntent());

    expect(screen.queryByText("Version")).not.toBeInTheDocument();
    expect(screen.queryByText("Trigger")).not.toBeInTheDocument();
    expect(screen.queryByText("Status")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Raw extravars")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Advanced/i }));

    expect(screen.getByLabelText("Raw extravars")).toBeInTheDocument();
  });

  it("resets edited launch options after closing and reopening the same intent", () => {
    const { onConfirm, rerenderDialog } = renderDialog(staticIntent());

    fireEvent.change(screen.getByPlaceholderText("900"), {
      target: { value: "123" },
    });
    fireEvent.click(screen.getByRole("button", { name: /Advanced/i }));
    fireEvent.change(screen.getByLabelText("Raw extravars"), {
      target: { value: '{"stale":true}' },
    });

    rerenderDialog(false);
    rerenderDialog(true);

    expect(screen.queryByLabelText("Raw extravars")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Queue run/i }));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    const submitted = onConfirm.mock.calls[0]?.[0] as PlaybookLaunchIntent;
    const launchOptions = (submitted.metadata as Record<string, unknown>)
      .launch_options as Record<string, unknown>;
    expect(submitted.timeoutSeconds).toBeNull();
    expect(launchOptions.timeout_seconds).toBeNull();
    expect(launchOptions.extravars).toEqual({ package: "htop" });
  });
});
