import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, onTestFailed, vi } from "vitest";
import "@/shared/i18n";

import BackupPage from "./BackupPage";
import { AllPermissions } from "@/test/permissions";
import type * as BackupApi from "@/features/backup/backup-api";
import type {
  FullBackupArchive,
  FullBackupImportSummary,
} from "@/features/backup/backup-api";

const apiMock = vi.hoisted(() => ({
  exportFullBackup: vi.fn(),
  importFullBackup: vi.fn(),
  readBackupFile: vi.fn(),
  downloadBackupFile: vi.fn(),
}));

vi.mock("@/features/backup/backup-api", async (importOriginal) => ({
  ...(await importOriginal<typeof BackupApi>()),
  ...apiMock,
}));

const notifyMock = vi.hoisted(() => vi.fn());
vi.mock("@/shared/lib/notifications", () => ({ notify: notifyMock }));

const archive: FullBackupArchive = {
  version: "piping-full-backup-v1",
  schemaVersion: 12,
  createdAt: "2026-07-01T00:00:00Z",
  includeSecrets: true,
  tableCount: 2,
  rowCount: 5,
  skipped: [],
  tables: [
    { name: "inventory_devices", section: "inventory", columns: [], rows: [] },
    { name: "access_users", section: "access", columns: [], rows: [] },
  ],
};

const preview: FullBackupImportSummary = {
  applied: false,
  tableCount: 2,
  rowCount: 5,
  mode: "full",
  plan: {
    dryRun: true,
    tableCount: 2,
    rowCount: 5,
    hasDestructiveChanges: false,
    warnings: [],
    tables: [
      {
        name: "inventory_devices",
        section: "inventory",
        action: "replace",
        rows: 3,
        inserts: 3,
        updates: 0,
        deletes: 0,
        unchanged: 0,
        policy: {
          updateColumns: [],
          preserveColumns: [],
          nullPolicy: "apply",
          jsonMergeColumns: [],
        },
        warnings: [],
      },
      {
        name: "access_users",
        section: "access",
        action: "replace",
        rows: 2,
        inserts: 1,
        updates: 1,
        deletes: 0,
        unchanged: 0,
        policy: {
          updateColumns: [],
          preserveColumns: [],
          nullPolicy: "apply",
          jsonMergeColumns: [],
        },
        warnings: [],
      },
    ],
  },
};

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <AllPermissions>
        <BackupPage />
      </AllPermissions>
    </QueryClientProvider>,
  );
}

async function openRestorePreview(container: HTMLElement, onStage?: (stage: string) => void) {
  onStage?.("selecting the archive");
  fireEvent.mouseDown(screen.getByRole("tab", { name: /restore/i }));

  const fileInput = container.querySelector('input[type="file"]');
  expect(fileInput).not.toBeNull();
  const file = new File([JSON.stringify(archive)], "backup.json", {
    type: "application/json",
  });
  await act(async () => {
    fireEvent.change(fileInput as HTMLInputElement, { target: { files: [file] } });
    await Promise.resolve();
  });

  const previewButton = screen.getByRole("button", {
    name: /preview restore/i,
  });
  onStage?.("waiting for the archive to enable preview");
  await waitFor(() => expect(previewButton, "Selected archive must be ready before clicking preview").toBeEnabled());
  onStage?.("starting the preview request");
  await act(async () => {
    fireEvent.click(previewButton);
    await Promise.resolve();
  });
  return file;
}

describe("BackupPage", () => {
  beforeEach(() => {
    Object.values(apiMock).forEach((mock) => mock.mockReset());
    notifyMock.mockReset();
    apiMock.readBackupFile.mockResolvedValue(archive);
    apiMock.importFullBackup.mockResolvedValue(preview);
  });

  it("renders the restore preview through the registered DataTable", async () => {
    const { container } = renderPage();
    await openRestorePreview(container);

    const tableRoot = await screen.findByText("inventory_devices");
    expect(tableRoot).toBeInTheDocument();

    const dataTableRoot = container.querySelector('[data-table-id="backup.preview"]');
    expect(dataTableRoot).not.toBeNull();

    const tables = container.querySelectorAll("table");
    expect(tables).toHaveLength(1);
    expect(dataTableRoot?.contains(tables[0] ?? null)).toBe(true);
    expect(screen.getByText("access_users")).toBeInTheDocument();
  });

  it("ignores an older archive read after a newer file was selected", async () => {
    let finishOld!: (value: FullBackupArchive) => void;
    apiMock.readBackupFile.mockImplementationOnce(() => new Promise<FullBackupArchive>(resolve => { finishOld = resolve; }));
    const { container } = renderPage();
    fireEvent.mouseDown(screen.getByRole("tab", { name: /restore/i }));
    const input = container.querySelector('input[type="file"]')!;
    fireEvent.change(input, { target: { files: [new File(["{}"], "old.json")] } });
    const latest = { ...archive, version: "latest-archive" };
    apiMock.readBackupFile.mockResolvedValueOnce(latest);
    fireEvent.change(input, { target: { files: [new File(["{}"], "latest.json")] } });
    expect(await screen.findByText(/latest-archive/)).toBeInTheDocument();
    await act(async () => { finishOld(archive); await Promise.resolve(); });
    expect(screen.getByText(/latest-archive/)).toBeInTheDocument();
    expect(screen.queryByText(/piping-full-backup-v1/)).not.toBeInTheDocument();
  });

  it("does not install a preview from a previously selected file with the same name and size", async () => {
    let stage = "rendering the backup page";
    onTestFailed(() => { console.error(`Backup stale-preview regression failed during: ${stage}`); });
    let finishPreview!: (value: FullBackupImportSummary) => void;
    apiMock.importFullBackup.mockImplementationOnce(() => new Promise(resolve => { finishPreview = resolve; }));
    const { container } = renderPage();
    const previousFile = await openRestorePreview(container, nextStage => { stage = nextStage; });
    expect(apiMock.importFullBackup, "The previous selection must have a pending preview request").toHaveBeenCalledTimes(1);
    expect(apiMock.importFullBackup).toHaveBeenCalledWith(expect.objectContaining({ payload: archive, dryRun: true, confirmFullRestore: false }));
    const input = container.querySelector('input[type="file"]')!;
    const nextFile = new File([JSON.stringify(archive)], "backup.json", { type: "application/json" });
    expect(nextFile).not.toBe(previousFile);
    expect(nextFile.name).toBe(previousFile.name);
    expect(nextFile.size).toBe(previousFile.size);
    stage = "reading the replacement selection";
    await act(async () => {
      fireEvent.change(input, { target: { files: [nextFile] } });
      await Promise.resolve();
    });
    expect(apiMock.readBackupFile, "The replacement selection must be read before resolving the old preview").toHaveBeenCalledTimes(2);
    expect(apiMock.readBackupFile).toHaveBeenLastCalledWith(nextFile);
    expect(screen.getByText(/piping-full-backup-v1/)).toBeInTheDocument();
    stage = "resolving the old preview request";
    await act(async () => { finishPreview(preview); await Promise.resolve(); });
    stage = "waiting for the old preview request to finish";
    await waitFor(() => expect(screen.getByRole("button", { name: /preview restore/i }), "The old preview request must finish before checking stale results").toBeEnabled());
    stage = "checking that the old preview was discarded";
    expect(notifyMock).not.toHaveBeenCalled();
    expect(screen.queryByText("inventory_devices")).not.toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("waits for the selected archive before requesting a restore preview", async () => {
    let finishRead!: (value: FullBackupArchive) => void;
    apiMock.readBackupFile.mockImplementationOnce(() => new Promise<FullBackupArchive>(resolve => { finishRead = resolve; }));
    const { container } = renderPage();
    const openingPreview = openRestorePreview(container);

    await waitFor(() => expect(apiMock.readBackupFile).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("button", { name: /preview restore/i })).toBeDisabled();
    expect(apiMock.importFullBackup).not.toHaveBeenCalled();

    await act(async () => { finishRead(archive); await Promise.resolve(); });
    await openingPreview;
    expect(apiMock.importFullBackup, "Preview must start after the archive read completes").toHaveBeenCalledTimes(1);
    expect(await screen.findByText("inventory_devices")).toBeInTheDocument();
  });
});
