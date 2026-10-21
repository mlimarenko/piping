import { describe, expect, it } from "vitest";

import { buildStaticPlaybookRunMetadata } from "@/features/connections/playbookLaunchMetadata";
import type { PlaybookLaunchIntent } from "@/features/playbooks/PlaybookLaunchDialog";

function intent(overrides: Partial<PlaybookLaunchIntent> = {}): PlaybookLaunchIntent {
  return {
    playbook: {
      playbook: {
        id: "playbook-1",
        name: "Gather facts",
        slug: "gather-facts",
        description: null,
        category: "inventory",
        runtime: "ansible",
        body: "---",
        vars_schema: [],
        version: 1,
        connection_default: "ssh",
        target_set_id: null,
        status: "active",
        risk: "low",
        timeout_seconds: 1800,
        source: "user",
        is_system: false,
        created_at: "2026-07-06T00:00:00Z",
        updated_at: "2026-07-06T00:00:00Z",
      },
      target_set: null,
    },
    triggerKind: "manual",
    ...overrides,
  } as PlaybookLaunchIntent;
}

describe("buildStaticPlaybookRunMetadata", () => {
  it("adds SSH launch options for workspace static runs", () => {
    expect(buildStaticPlaybookRunMetadata(intent(), "connections.quick_action", 7)).toEqual({
      source: "connections.quick_action",
      target_count: 7,
      launch_options: {
        connection: "ssh",
        credential_choice: "auto",
        target_limit: 7,
      },
    });
  });

  it("preserves dialog launch options and only fills missing defaults", () => {
    const metadata = buildStaticPlaybookRunMetadata(
      intent({
        targetLimit: 12,
        credentialChoice: "personal",
        metadata: {
          source: "old-source",
          target_count: 1,
          launch_options: {
            connection: "winrm",
            credential_choice: "service-account",
            target_limit: 3,
            dry_run: true,
            extravars: { collect_hardware: false },
          },
        },
      }),
      "connections.bulk_launch",
      12,
    );

    expect(metadata).toEqual({
      source: "connections.bulk_launch",
      target_count: 12,
      launch_options: {
        connection: "winrm",
        credential_choice: "service-account",
        target_limit: 3,
        dry_run: true,
        extravars: { collect_hardware: false },
      },
    });
  });

  it("normalizes empty connection values to the playbook default", () => {
    expect(
      buildStaticPlaybookRunMetadata(
        intent({ metadata: { launch_options: { connection: "" } } }),
        "connections.quick_action",
        1,
      ),
    ).toMatchObject({
      launch_options: {
        connection: "ssh",
      },
    });
  });
});
