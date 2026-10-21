import { afterEach, describe, expect, it, vi } from "vitest";

import {
  importFullBackup,
  type FullBackupArchive,
  type FullBackupImportSummary,
} from "./backup-api";

const archive: FullBackupArchive = {
  version: "piping-full-backup-v1",
  schemaVersion: 1,
  createdAt: "2026-07-08T00:00:00Z",
  includeSecrets: true,
  tableCount: 1,
  rowCount: 1,
  skipped: [],
  tables: [
    {
      name: "app_setting",
      section: "settings",
      columns: [{ name: "id", dataType: "uuid" }],
      rows: [{ id: "00000000-0000-0000-0000-000000000001" }],
    },
  ],
};

function response(summary: FullBackupImportSummary): Response {
  return new Response(JSON.stringify(summary), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

describe("backup import API", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("sends dry-run restore previews without full-restore confirmation", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      response({
        applied: false,
        tableCount: 1,
        rowCount: 1,
        mode: "selective",
        plan: {
          dryRun: true,
          tableCount: 1,
          rowCount: 1,
          hasDestructiveChanges: false,
          warnings: [],
          tables: [],
        },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const summary = await importFullBackup({
      payload: archive,
      dryRun: true,
      confirmFullRestore: false,
      selection: { sections: ["settings"] },
      rules: { defaultAction: "skip", sections: { settings: "replace" } },
    });

    expect(summary.plan?.dryRun).toBe(true);
    const request = fetchMock.mock.calls[0]?.[0] as Request;
    expect(request.method).toBe("POST");
    expect(request.credentials).toBe("include");
    expect(new URL(request.url).pathname).toBe("/api/v1/admin/backup/import");
    expect(await request.clone().json()).toMatchObject({
      dryRun: true,
      confirmFullRestore: false,
      selection: { sections: ["settings"] },
      rules: { defaultAction: "skip", sections: { settings: "replace" } },
    });
  });

  it("confirms apply requests by default", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      response({
        applied: true,
        tableCount: 1,
        rowCount: 1,
        mode: "full",
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await importFullBackup({ payload: archive });

    const request = fetchMock.mock.calls[0]?.[0] as Request;
    const requestBody = await request.clone().json();
    expect(requestBody.dryRun).toBe(false);
    expect(requestBody.confirmFullRestore).toBe(true);
  });
});
