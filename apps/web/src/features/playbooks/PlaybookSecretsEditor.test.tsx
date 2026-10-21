import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";

import PlaybookSecretsEditor from "./PlaybookSecretsEditor";
import type { AutomationSecret } from "@/features/automation-secrets/api";
import type { SecretEnvRow } from "@/features/playbooks/secretsPolicy";

const secret = (over: Partial<AutomationSecret>): AutomationSecret => ({
  id: "s1",
  key: "grad-token",
  description: null,
  has_value: true,
  is_masked: true,
  usage_count: 0,
  created_at: "2026-07-01T00:00:00Z",
  ...over,
});

const row = (over: Partial<SecretEnvRow>): SecretEnvRow => ({
  id: "r1",
  envName: "",
  key: "",
  ...over,
});

describe("PlaybookSecretsEditor", () => {
  it("shows the empty state and reserved-prefix hint, and adds a row", () => {
    const onChange = vi.fn();
    render(
      <PlaybookSecretsEditor rows={[]} onChange={onChange} secrets={[]} />,
    );

    expect(screen.getByText(/no secrets declared/i)).toBeInTheDocument();
    expect(screen.getByText(/PIPING_/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /add secret/i }));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange.mock.calls[0]![0]).toHaveLength(1);
  });

  it("flags a reserved env name", () => {
    render(
      <PlaybookSecretsEditor
        rows={[row({ envName: "ANSIBLE_TOKEN", key: "grad-token" })]}
        onChange={vi.fn()}
        secrets={[secret({})]}
      />,
    );
    expect(screen.getByText(/this name is reserved/i)).toBeInTheDocument();
  });

  it("flags a malformed env name", () => {
    render(
      <PlaybookSecretsEditor
        rows={[row({ envName: "bad-name" })]}
        onChange={vi.fn()}
        secrets={[]}
      />,
    );
    expect(screen.getByText(/cannot start with a digit/i)).toBeInTheDocument();
  });

  it("badges a selected secret that has no value stored", () => {
    render(
      <PlaybookSecretsEditor
        rows={[row({ envName: "GRAD_TOKEN", key: "grad-token" })]}
        onChange={vi.fn()}
        secrets={[secret({ has_value: false })]}
      />,
    );
    expect(screen.getByText(/no value set/i)).toBeInTheDocument();
  });

  it("removes a row", () => {
    const onChange = vi.fn();
    render(
      <PlaybookSecretsEditor
        rows={[
          row({ id: "r1", envName: "A", key: "grad-token" }),
          row({ id: "r2", envName: "B", key: "grad-token" }),
        ]}
        onChange={onChange}
        secrets={[secret({})]}
      />,
    );
    fireEvent.click(screen.getAllByRole("button", { name: /remove secret/i })[0]!);
    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0]![0] as { id: string }[];
    expect(next).toHaveLength(1);
    expect(next[0]!.id).toBe("r2");
  });
});
