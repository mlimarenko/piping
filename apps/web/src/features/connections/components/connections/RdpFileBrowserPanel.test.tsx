import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@/shared/i18n";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RdpFileBrowserPanel from "./RdpFileBrowserPanel";
import type { RdpFilesystem } from "@/features/connections/adapters/ws/rdpFilesystem";
import type { LiveSession } from "@/features/connections/ConnectionsWorkspace";

const session: LiveSession = {
  id: "session-1",
  hostId: "host-1",
  hostName: "win-rdp-jump",
  protocol: "rdp",
  status: "connected",
  websocketUrl: "",
  token: "",
};

function makeFilesystem() {
  const upload = vi.fn((
    _path: string,
    _file: File,
    onProgress?: (loaded: number, total: number) => void,
  ): Promise<void> => {
    onProgress?.(7, 7);
    return Promise.resolve();
  });
  const filesystem: RdpFilesystem = {
    name: "Piping",
    list: vi.fn().mockResolvedValue([]),
    download: vi.fn(),
    upload,
  };
  return { filesystem, upload };
}

describe("RdpFileBrowserPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("labels guacd's generic shared drive with the Windows-visible Piping location", async () => {
    const { filesystem } = makeFilesystem();
    Object.defineProperty(filesystem, "name", { value: "Shared Drive" });

    render(<RdpFileBrowserPanel session={session} filesystem={filesystem} />);

    expect(screen.getByText("This PC -> Piping on Guacamole · win-rdp-jump")).toBeInTheDocument();
    expect(await screen.findByText("The drive is empty")).toBeInTheDocument();
    expect(screen.getByText("Files copied to Piping on Guacamole in the remote session appear here.")).toBeInTheDocument();
  });

  it("uploads dropped files into the current redirected-drive directory", async () => {
    const { filesystem, upload } = makeFilesystem();
    render(<RdpFileBrowserPanel session={session} filesystem={filesystem} />);
    expect(await screen.findByText("The drive is empty")).toBeInTheDocument();

    const panel = screen.getByTestId("rdp-file-browser");
    const file = new File(["payload"], "artifact.txt", { type: "text/plain" });

    fireEvent.dragOver(panel, {
      dataTransfer: { files: [file], items: [] },
    });
    expect(screen.getByText("Drop files to upload to /")).toBeInTheDocument();

    fireEvent.drop(panel, {
      dataTransfer: { files: [file], items: [] },
    });

    await waitFor(() => {
      expect(upload).toHaveBeenCalledWith("/", file, expect.any(Function));
    });
    expect(screen.queryByText("Drop files to upload to /")).not.toBeInTheDocument();
    expect(screen.getByText("artifact.txt")).toBeInTheDocument();
  });
});
