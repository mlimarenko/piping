import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import RecordForm from "../RecordForm";
import { DEFAULT_ENDPOINT_HOST, type RecordDraft } from "../recordDraft";
import type { ConnConfParams } from "../api";

function makeDraft(overrides: Partial<RecordDraft> = {}): RecordDraft {
  return {
    name: "cam-1",
    description: "",
    protocol: "ssh",
    baseRecordId: "",
    enabled: true,
    params: { host: DEFAULT_ENDPOINT_HOST },
    ...overrides,
  };
}

function renderForm(draft: RecordDraft, onChange = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const utils = render(
    <QueryClientProvider client={client}>
      <RecordForm draft={draft} onChange={onChange} baseRecords={[]} />
    </QueryClientProvider>,
  );
  return { onChange, ...utils };
}

describe("RecordForm — cctv protocol", () => {
  it("renders the cctv-specific fields and omits port/timeout", () => {
    renderForm(makeDraft({ protocol: "cctv" }));

    // Video-only fields (inline-fallback labels until locale entries land).
    expect(screen.getByText("Management port")).toBeInTheDocument();
    expect(screen.getByText("Channel")).toBeInTheDocument();
    expect(screen.getByText("Family")).toBeInTheDocument();
    // Shared credential fields still render.
    expect(screen.getByText("Username")).toBeInTheDocument();
    expect(screen.getByText("Password")).toBeInTheDocument();

    // Session-only fields must NOT appear for video.
    expect(screen.queryByText("Port")).not.toBeInTheDocument();
    expect(screen.queryByText("Timeout (s)")).not.toBeInTheDocument();
  });

  it("switching a session record to cctv yields video-legal params (no port/timeout_secs)", () => {
    const draft = makeDraft({
      protocol: "ssh",
      params: {
        host: "10.0.0.9",
        port: 22,
        timeout_secs: 30,
        credentials: { username: "admin", password: "secret" },
      },
    });
    let captured: RecordDraft | undefined;
    const onChange = vi.fn((next: RecordDraft) => {
      captured = next;
    });
    renderForm(draft, onChange);

    fireEvent.click(screen.getByRole("combobox", { name: "Protocol" }));
    fireEvent.click(screen.getByRole("option", { name: "CCTV" }));

    expect(onChange).toHaveBeenCalledTimes(1);
    expect(captured?.protocol).toBe("cctv");
    const params: ConnConfParams = captured?.params ?? {};
    expect(params.host).toBe("10.0.0.9");
    expect(params).not.toHaveProperty("port");
    expect(params).not.toHaveProperty("timeout_secs");
  });
});
