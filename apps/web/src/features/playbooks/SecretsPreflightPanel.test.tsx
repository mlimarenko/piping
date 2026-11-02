import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import "@/shared/i18n";

import SecretsPreflightPanel from "./SecretsPreflightPanel";
import {
  isPreflightRowBlocking,
  type SecretPreflightEntry,
} from "@/features/automation-secrets/api";

const entry = (over: Partial<SecretPreflightEntry>): SecretPreflightEntry => ({
  env_name: "GRAD_TOKEN",
  key: "grad-token",
  secret_id: "s1",
  has_value: true,
  allowed: true,
  ...over,
});

describe("isPreflightRowBlocking", () => {
  it("blocks on missing access or missing value", () => {
    expect(isPreflightRowBlocking(entry({}))).toBe(false);
    expect(isPreflightRowBlocking(entry({ allowed: false }))).toBe(true);
    expect(isPreflightRowBlocking(entry({ has_value: false }))).toBe(true);
  });
});

describe("SecretsPreflightPanel", () => {
  it("renders nothing when no secrets are declared", () => {
    const { container } = render(<SecretsPreflightPanel entries={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("lists granted secrets without a blocking hint", () => {
    render(<SecretsPreflightPanel entries={[entry({})]} />);
    expect(screen.getByText("GRAD_TOKEN")).toBeInTheDocument();
    expect(screen.queryByText(/administrator/i)).not.toBeInTheDocument();
  });

  it("surfaces a no-access verdict and a blocking hint", () => {
    render(
      <SecretsPreflightPanel
        entries={[
          entry({ env_name: "GRAD_TOKEN" }),
          entry({ env_name: "EXT_KEY", key: "ext", allowed: false }),
        ]}
      />,
    );
    expect(screen.getByText(/no access/i)).toBeInTheDocument();
    expect(screen.getByText(/administrator/i)).toBeInTheDocument();
  });

  it("surfaces a missing-value verdict", () => {
    render(
      <SecretsPreflightPanel entries={[entry({ has_value: false })]} />,
    );
    expect(screen.getByText(/value not set/i)).toBeInTheDocument();
  });
});
