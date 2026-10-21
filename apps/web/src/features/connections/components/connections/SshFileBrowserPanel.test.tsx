import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";
import SshFileBrowserPanel from "./SshFileBrowserPanel";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";
import {
  deleteSshSessionFiles,
  ensureSshSessionDirectory,
  listSshSessionFiles,
  moveSshSessionFiles,
  preflightSshSessionFileDownload,
  previewSshSessionFile,
  startSshSessionFileDownload,
  uploadSshSessionFile,
  type SshFileList,
} from "@/features/connections/session-files-api";

vi.mock("@/features/connections/session-files-api", () => ({
  deleteSshSessionFiles: vi.fn(),
  ensureSshSessionDirectory: vi.fn(),
  listSshSessionFiles: vi.fn(),
  moveSshSessionFiles: vi.fn(),
  preflightSshSessionFileDownload: vi.fn(),
  previewSshSessionFile: vi.fn(),
  startSshSessionFileDownload: vi.fn(),
  uploadSshSessionFile: vi.fn(),
}));

const session: LiveSession = {
  id: "session-1",
  hostId: "host-1",
  hostName: "pxm-docker01",
  protocol: "ssh",
  status: "connected",
  websocketUrl: "",
  token: "",
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function listing(path: string, names: string[]): SshFileList {
  return {
    path,
    parent: path === "/" ? null : "/",
    entries: names.map((name) => ({
      name,
      path: `${path.replace(/\/+$/, "")}/${name}`,
      kind: "file",
      size: 128,
      modified_at: "2026-06-04T10:00:00.000Z",
    })),
  };
}

function listingWithEntries(path: string, entries: SshFileList["entries"]): SshFileList {
  return {
    path,
    parent: path === "/" ? null : "/",
    entries,
  };
}

describe("SshFileBrowserPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(ensureSshSessionDirectory).mockResolvedValue({
      path: "/ignored",
      bytes: 0,
      duration_ms: 0,
    });
    vi.mocked(preflightSshSessionFileDownload).mockResolvedValue({
      path: "/ignored",
      file_name: "ignored",
      bytes: 0,
      logical_bytes: 0,
      entry_count: 1,
      kind: "file",
      archive: false,
    });
    vi.mocked(startSshSessionFileDownload).mockImplementation(() => {});
    vi.mocked(previewSshSessionFile).mockResolvedValue({
      path: "/ignored",
      bytes: 0,
      preview_bytes: 0,
      truncated: false,
      content: "",
    });
    vi.mocked(deleteSshSessionFiles).mockResolvedValue({
      paths: [],
      count: 0,
      duration_ms: 0,
    });
    vi.mocked(moveSshSessionFiles).mockResolvedValue({
      paths: [],
      destination: "/tmp",
      count: 0,
      duration_ms: 0,
    });
  });

  it("ignores stale directory responses from earlier navigation", async () => {
    const listMock = vi.mocked(listSshSessionFiles);
    vi.mocked(uploadSshSessionFile).mockResolvedValue({
      path: "/ignored",
      bytes: 0,
      duration_ms: 0,
    });

    const slow = deferred<SshFileList>();
    const fast = deferred<SshFileList>();
    listMock
      .mockResolvedValueOnce(listing("/home", ["initial.txt"]))
      .mockReturnValueOnce(slow.promise)
      .mockReturnValueOnce(fast.promise);

    render(<SshFileBrowserPanel session={session} />);

    expect(await screen.findByText("initial.txt")).toBeInTheDocument();

    const pathInput = screen.getByRole("textbox", { name: "Remote path" });
    const goButton = screen.getByRole("button", { name: "Go" });

    fireEvent.change(pathInput, { target: { value: "/slow" } });
    fireEvent.click(goButton);
    fireEvent.change(pathInput, { target: { value: "/fast" } });
    fireEvent.click(goButton);

    fast.resolve(listing("/fast", ["fast.txt"]));
    expect(await screen.findByText("fast.txt")).toBeInTheDocument();

    slow.resolve(listing("/slow", ["slow.txt"]));

    await waitFor(() => {
      expect(screen.getByText("fast.txt")).toBeInTheDocument();
      expect(screen.queryByText("slow.txt")).not.toBeInTheDocument();
      expect(pathInput).toHaveValue("/fast");
    });
  });

  it("does not refresh the upload start directory after the user navigates away", async () => {
    const listMock = vi.mocked(listSshSessionFiles);
    const uploadMock = vi.mocked(uploadSshSessionFile);
    const upload = deferred<{ path: string; bytes: number; duration_ms: number }>();
    uploadMock.mockReturnValue(upload.promise);
    listMock
      .mockResolvedValueOnce(listing("/root", ["root.txt"]))
      .mockResolvedValueOnce(listing("/etc", ["etc.conf"]));

    const { container } = render(<SshFileBrowserPanel session={session} />);
    expect(await screen.findByText("root.txt")).toBeInTheDocument();

    const fileInput = container.querySelector<HTMLInputElement>('input[type="file"]');
    expect(fileInput).not.toBeNull();
    const file = new File(["payload"], "artifact.txt", { type: "text/plain" });
    fireEvent.change(fileInput!, { target: { files: [file] } });
    await waitFor(() => expect(uploadMock).toHaveBeenCalledOnce());

    const pathInput = screen.getByRole("textbox", { name: "Remote path" });
    const goButton = screen.getByRole("button", { name: "Go" });
    fireEvent.change(pathInput, { target: { value: "/etc" } });
    fireEvent.click(goButton);

    expect(await screen.findByText("etc.conf")).toBeInTheDocument();
    expect(pathInput).toHaveValue("/etc");

    upload.resolve({
      path: "/root/artifact.txt",
      bytes: 7,
      duration_ms: 12,
    });

    await waitFor(() => {
      expect(screen.getByText("etc.conf")).toBeInTheDocument();
      expect(screen.queryByText("root.txt")).not.toBeInTheDocument();
      expect(pathInput).toHaveValue("/etc");
      expect(listMock).toHaveBeenCalledTimes(2);
    });
  });

  it("refreshes the current directory after overlapping upload batches finish", async () => {
    const listMock = vi.mocked(listSshSessionFiles);
    const uploadMock = vi.mocked(uploadSshSessionFile);
    const firstUpload = deferred<{ path: string; bytes: number; duration_ms: number }>();
    const secondUpload = deferred<{ path: string; bytes: number; duration_ms: number }>();
    uploadMock.mockReturnValueOnce(firstUpload.promise).mockReturnValueOnce(secondUpload.promise);
    listMock
      .mockResolvedValueOnce(listing("/root", ["root.txt"]))
      .mockResolvedValueOnce(listing("/root", ["root.txt", "first.txt"]))
      .mockResolvedValueOnce(listing("/root", ["root.txt", "first.txt", "second.txt"]));

    const { container } = render(<SshFileBrowserPanel session={session} />);
    expect(await screen.findByText("root.txt")).toBeInTheDocument();

    const fileInput = container.querySelector<HTMLInputElement>('input[type="file"]');
    expect(fileInput).not.toBeNull();

    fireEvent.change(fileInput!, {
      target: { files: [new File(["first"], "first.txt", { type: "text/plain" })] },
    });
    await waitFor(() => expect(uploadMock).toHaveBeenCalledTimes(1));

    fireEvent.change(fileInput!, {
      target: { files: [new File(["second"], "second.txt", { type: "text/plain" })] },
    });
    await waitFor(() => expect(uploadMock).toHaveBeenCalledTimes(2));

    firstUpload.resolve({
      path: "/root/first.txt",
      bytes: 5,
      duration_ms: 10,
    });
    expect(await screen.findByText("first.txt")).toBeInTheDocument();
    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));

    secondUpload.resolve({
      path: "/root/second.txt",
      bytes: 6,
      duration_ms: 12,
    });

    await waitFor(() => {
      expect(listMock).toHaveBeenCalledTimes(3);
      expect(screen.getAllByText("second.txt").length).toBeGreaterThanOrEqual(2);
    });
  });

  it("refreshes the upload directory when pending navigation fails after upload completion", async () => {
    const listMock = vi.mocked(listSshSessionFiles);
    const uploadMock = vi.mocked(uploadSshSessionFile);
    const upload = deferred<{ path: string; bytes: number; duration_ms: number }>();
    const failedNavigation = deferred<SshFileList>();
    uploadMock.mockReturnValue(upload.promise);
    listMock
      .mockResolvedValueOnce(listing("/root", ["root.txt"]))
      .mockReturnValueOnce(failedNavigation.promise)
      .mockResolvedValueOnce(listing("/root", ["root.txt", "artifact.txt"]));

    const { container } = render(<SshFileBrowserPanel session={session} />);
    expect(await screen.findByText("root.txt")).toBeInTheDocument();

    const fileInput = container.querySelector<HTMLInputElement>('input[type="file"]');
    expect(fileInput).not.toBeNull();
    fireEvent.change(fileInput!, {
      target: { files: [new File(["payload"], "artifact.txt", { type: "text/plain" })] },
    });
    await waitFor(() => expect(uploadMock).toHaveBeenCalledOnce());

    const pathInput = screen.getByRole("textbox", { name: "Remote path" });
    const goButton = screen.getByRole("button", { name: "Go" });
    fireEvent.change(pathInput, { target: { value: "/etc" } });
    fireEvent.click(goButton);
    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));

    upload.resolve({
      path: "/root/artifact.txt",
      bytes: 7,
      duration_ms: 12,
    });
    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));

    failedNavigation.reject(new Error("navigation failed"));

    await waitFor(() => {
      expect(listMock).toHaveBeenCalledTimes(3);
      expect(screen.getAllByText("artifact.txt").length).toBeGreaterThanOrEqual(2);
      expect(pathInput).toHaveValue("/root");
    });
  });

  it("refreshes after a pending alias navigation resolves to the upload directory", async () => {
    const listMock = vi.mocked(listSshSessionFiles);
    const uploadMock = vi.mocked(uploadSshSessionFile);
    const upload = deferred<{ path: string; bytes: number; duration_ms: number }>();
    const staleHomeLoad = deferred<SshFileList>();
    uploadMock.mockReturnValue(upload.promise);
    listMock
      .mockResolvedValueOnce(listing("/root", ["root.txt"]))
      .mockReturnValueOnce(staleHomeLoad.promise)
      .mockResolvedValueOnce(listing("/root", ["root.txt", "artifact.txt"]));

    const { container } = render(<SshFileBrowserPanel session={session} />);
    expect(await screen.findByText("root.txt")).toBeInTheDocument();

    const fileInput = container.querySelector<HTMLInputElement>('input[type="file"]');
    expect(fileInput).not.toBeNull();
    fireEvent.change(fileInput!, {
      target: { files: [new File(["payload"], "artifact.txt", { type: "text/plain" })] },
    });
    await waitFor(() => expect(uploadMock).toHaveBeenCalledOnce());

    fireEvent.click(screen.getByRole("button", { name: "Home" }));
    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));

    upload.resolve({
      path: "/root/artifact.txt",
      bytes: 7,
      duration_ms: 12,
    });
    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));

    staleHomeLoad.resolve(listing("/root", ["root.txt"]));

    await waitFor(() => {
      expect(listMock).toHaveBeenCalledTimes(3);
      expect(screen.getAllByText("artifact.txt").length).toBeGreaterThanOrEqual(2);
    });
  });

  it("selects a file and renders compact details", async () => {
    vi.mocked(previewSshSessionFile).mockResolvedValueOnce({
      path: "/root/.ssh/authorized_keys",
      bytes: 24,
      preview_bytes: 24,
      truncated: false,
      content: "ssh-ed25519 AAAA",
    });
    vi.mocked(listSshSessionFiles).mockResolvedValueOnce(
      listingWithEntries("/root", [
        {
          name: "authorized_keys",
          path: "/root/.ssh/authorized_keys",
          kind: "file",
          size: 42,
          modified_at: "2026-06-04T10:00:00.000Z",
          permissions: "0600",
        },
      ]),
    );

    render(<SshFileBrowserPanel session={session} />);

    fireEvent.click(await screen.findByText("authorized_keys"));

    const details = screen.getByRole("complementary", { name: "Details" });
    expect(details).toHaveTextContent("/root/.ssh/authorized_keys");
    expect(details).toHaveTextContent("0600");
    expect(details).toHaveTextContent("42 B");
    expect(await screen.findByText("ssh-ed25519 AAAA")).toBeInTheDocument();
    expect(previewSshSessionFile).toHaveBeenCalledWith("session-1", "/root/.ssh/authorized_keys");
  });

  it("selects a file from the keyboard and exposes details actions", async () => {
    vi.mocked(listSshSessionFiles).mockResolvedValueOnce(
      listingWithEntries("/root", [
        {
          name: "known_hosts",
          path: "/root/.ssh/known_hosts",
          kind: "file",
          size: 84,
          modified_at: "2026-06-04T10:00:00.000Z",
          permissions: "0644",
        },
      ]),
    );

    render(<SshFileBrowserPanel session={session} />);

    const row = await screen.findByRole("row", { name: "known_hosts" });
    row.focus();
    fireEvent.keyDown(row, { key: "Enter" });

    const details = screen.getByRole("complementary", { name: "Details" });
    expect(details).toHaveTextContent("/root/.ssh/known_hosts");
    expect(screen.getByRole("button", { name: "Copy path" })).toBeInTheDocument();
  });

  it("downloads directories as archives through the file transfer API", async () => {
    const preflightMock = vi.mocked(preflightSshSessionFileDownload);
    const startMock = vi.mocked(startSshSessionFileDownload);
    vi.mocked(listSshSessionFiles).mockResolvedValueOnce(
      listingWithEntries("/root", [
        {
          name: "reports",
          path: "/root/reports",
          kind: "directory",
          size: 4096,
          modified_at: "2026-06-04T10:00:00.000Z",
        },
      ]),
    );
    preflightMock.mockResolvedValueOnce({
      path: "/root/reports",
      file_name: "reports.tar.gz",
      bytes: null,
      logical_bytes: 8192,
      entry_count: 3,
      kind: "directory",
      archive: true,
    });

    render(<SshFileBrowserPanel session={session} />);
    expect(await screen.findByText("reports")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Download folder" }));

    await waitFor(() => {
      expect(preflightMock).toHaveBeenCalledWith("session-1", "/root/reports", {
        archive: true,
      });
      expect(startMock).toHaveBeenCalledWith("session-1", "/root/reports", "reports.tar.gz", {
        archive: true,
      });
    });
  });

  it("downloads selected files through the selected action toolbar", async () => {
    const preflightMock = vi.mocked(preflightSshSessionFileDownload);
    const startMock = vi.mocked(startSshSessionFileDownload);
    vi.mocked(listSshSessionFiles).mockResolvedValueOnce(
      listingWithEntries("/root", [
        {
          name: "alpha.txt",
          path: "/root/alpha.txt",
          kind: "file",
          size: 5,
          modified_at: "2026-06-04T10:00:00.000Z",
        },
      ]),
    );
    preflightMock.mockResolvedValueOnce({
      path: "/root/alpha.txt",
      file_name: "alpha.txt",
      bytes: 5,
      logical_bytes: 5,
      entry_count: 1,
      kind: "file",
      archive: false,
    });

    render(<SshFileBrowserPanel session={session} />);
    expect(await screen.findByText("alpha.txt")).toBeInTheDocument();

    expect(screen.queryByRole("checkbox", { name: "Select alpha.txt" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Select files" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select alpha.txt" }));
    fireEvent.click(screen.getByRole("button", { name: "Download selected" }));

    await waitFor(() => {
      expect(preflightMock).toHaveBeenCalledWith("session-1", "/root/alpha.txt", {
        archive: false,
      });
      expect(startMock).toHaveBeenCalledWith("session-1", "/root/alpha.txt", "alpha.txt", {
        archive: false,
      });
    });
  });

  it("moves selected files through the selected action toolbar", async () => {
    const moveMock = vi.mocked(moveSshSessionFiles);
    vi.mocked(listSshSessionFiles)
      .mockResolvedValueOnce(
        listingWithEntries("/root", [
          {
            name: "alpha.txt",
            path: "/root/alpha.txt",
            kind: "file",
            size: 5,
            modified_at: "2026-06-04T10:00:00.000Z",
          },
        ]),
      )
      .mockResolvedValueOnce(listingWithEntries("/root", []));
    moveMock.mockResolvedValueOnce({
      paths: ["/root/alpha.txt"],
      destination: "/tmp/archive",
      count: 1,
      duration_ms: 12,
    });

    render(<SshFileBrowserPanel session={session} />);
    expect(await screen.findByText("alpha.txt")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Select files" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select alpha.txt" }));
    fireEvent.click(screen.getByRole("button", { name: "Move selected" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Destination folder" }), {
      target: { value: "/tmp/archive" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Move" }));

    await waitFor(() => {
      expect(moveMock).toHaveBeenCalledWith("session-1", ["/root/alpha.txt"], "/tmp/archive");
    });
  });

  it("moves the details entry through the selected action flow", async () => {
    const moveMock = vi.mocked(moveSshSessionFiles);
    vi.mocked(listSshSessionFiles)
      .mockResolvedValueOnce(
        listingWithEntries("/root", [
          {
            name: "alpha.txt",
            path: "/root/alpha.txt",
            kind: "file",
            size: 5,
            modified_at: "2026-06-04T10:00:00.000Z",
          },
        ]),
      )
      .mockResolvedValueOnce(listingWithEntries("/root", []));
    moveMock.mockResolvedValueOnce({
      paths: ["/root/alpha.txt"],
      destination: "/srv/archive",
      count: 1,
      duration_ms: 12,
    });

    render(<SshFileBrowserPanel session={session} />);
    fireEvent.click(await screen.findByText("alpha.txt"));

    fireEvent.click(screen.getByRole("button", { name: "Move" }));

    fireEvent.change(screen.getByRole("textbox", { name: "Destination folder" }), {
      target: { value: "/srv/archive" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Move" }));

    await waitFor(() => {
      expect(moveMock).toHaveBeenCalledWith("session-1", ["/root/alpha.txt"], "/srv/archive");
    });
  });

  it("deletes selected files after confirmation", async () => {
    const deleteMock = vi.mocked(deleteSshSessionFiles);
    vi.mocked(listSshSessionFiles)
      .mockResolvedValueOnce(
        listingWithEntries("/root", [
          {
            name: "alpha.txt",
            path: "/root/alpha.txt",
            kind: "file",
            size: 5,
            modified_at: "2026-06-04T10:00:00.000Z",
          },
        ]),
      )
      .mockResolvedValueOnce(listingWithEntries("/root", []));
    deleteMock.mockResolvedValueOnce({
      paths: ["/root/alpha.txt"],
      count: 1,
      duration_ms: 12,
    });

    render(<SshFileBrowserPanel session={session} />);
    expect(await screen.findByText("alpha.txt")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Select files" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Select alpha.txt" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete selected" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(deleteMock).toHaveBeenCalledWith("session-1", ["/root/alpha.txt"]);
    });
  });

  it("deletes the details entry through the selected action flow", async () => {
    const deleteMock = vi.mocked(deleteSshSessionFiles);
    vi.mocked(listSshSessionFiles)
      .mockResolvedValueOnce(
        listingWithEntries("/root", [
          {
            name: "alpha.txt",
            path: "/root/alpha.txt",
            kind: "file",
            size: 5,
            modified_at: "2026-06-04T10:00:00.000Z",
          },
        ]),
      )
      .mockResolvedValueOnce(listingWithEntries("/root", []));
    deleteMock.mockResolvedValueOnce({
      paths: ["/root/alpha.txt"],
      count: 1,
      duration_ms: 12,
    });

    render(<SshFileBrowserPanel session={session} />);
    fireEvent.click(await screen.findByText("alpha.txt"));

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    fireEvent.click(screen.getByRole("button", { name: "Delete" }));

    await waitFor(() => {
      expect(deleteMock).toHaveBeenCalledWith("session-1", ["/root/alpha.txt"]);
    });
  });

  it("uploads picked folders by creating remote directories before file transfers", async () => {
    const listMock = vi.mocked(listSshSessionFiles);
    const mkdirMock = vi.mocked(ensureSshSessionDirectory);
    const uploadMock = vi.mocked(uploadSshSessionFile);
    listMock
      .mockResolvedValueOnce(listing("/root", ["root.txt"]))
      .mockResolvedValueOnce(listing("/root", ["root.txt", "bundle"]));
    uploadMock.mockResolvedValue({
      path: "/ignored",
      bytes: 7,
      duration_ms: 12,
    });

    const { container } = render(<SshFileBrowserPanel session={session} />);
    expect(await screen.findByText("root.txt")).toBeInTheDocument();

    const directoryInput = container.querySelectorAll<HTMLInputElement>('input[type="file"]')[1];
    expect(directoryInput).toBeDefined();
    const first = new File(["alpha"], "alpha.txt", { type: "text/plain" });
    const second = new File(["beta"], "beta.txt", { type: "text/plain" });
    Object.defineProperty(first, "webkitRelativePath", { value: "bundle/alpha.txt" });
    Object.defineProperty(second, "webkitRelativePath", { value: "bundle/nested/beta.txt" });

    fireEvent.change(directoryInput!, {
      target: { files: [first, second] },
    });

    await waitFor(() => {
      expect(mkdirMock).toHaveBeenCalledWith("session-1", "/root/bundle");
      expect(mkdirMock).toHaveBeenCalledWith("session-1", "/root/bundle/nested");
    });
    await waitFor(() => {
      expect(uploadMock).toHaveBeenCalledWith(
        "session-1",
        "/root/bundle/alpha.txt",
        first,
        expect.any(Object),
      );
      expect(uploadMock).toHaveBeenCalledWith(
        "session-1",
        "/root/bundle/nested/beta.txt",
        second,
        expect.any(Object),
      );
    });
  });
});
