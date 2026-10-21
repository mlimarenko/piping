import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { InventoryTemplate } from "./templateTypes";
import TemplatePreviewRail from "./TemplatePreviewRail";

const previewTemplateRun = vi.fn();

vi.mock("react-i18next", () => ({
  initReactI18next: { type: "3rdParty", init: vi.fn() },
  useTranslation: () => ({
    t: (key: string, options?: { count?: number }) =>
      options?.count === undefined ? key : `${key}:${options.count}`,
  }),
}));

vi.mock("@/features/inventory/api", () => ({
  previewTemplateRun: (...args: unknown[]) => previewTemplateRun(...args),
}));

const template: InventoryTemplate = {
  id: "template-1",
  name: "Provision site",
  slug: "provision-site",
  status: "draft",
  description: "",
  updatedAt: "",
  variables: [],
  steps: [
    {
      step_key: "site",
      title: "Create site",
      resource_type: "site",
      operation: "create",
      depends_on_step_keys: [],
      payload_template: { name: "Berlin" },
      on_conflict_policy: "fail",
    },
  ],
};

describe("TemplatePreviewRail", () => {
  beforeEach(() => {
    previewTemplateRun.mockReset();
  });

  it("offers an explicit read retry without exposing backend text", async () => {
    previewTemplateRun.mockRejectedValueOnce(new Error("private backend trace")).mockResolvedValue({ steps: [] });
    render(<TemplatePreviewRail template={template} templateId="template-1" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("inventory.templates.run.previewError");
    expect(screen.queryByText("private backend trace")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "inventory.templates.run.retry" }));
    await waitFor(() => expect(previewTemplateRun).toHaveBeenCalledTimes(2), { timeout: 1500 });
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument());
  });

  it("renders a local preview for an unsaved template", () => {
    render(<TemplatePreviewRail template={template} templateId={null} />);

    expect(screen.getByText("Create site")).toBeInTheDocument();
    expect(screen.getByText("Berlin")).toBeInTheDocument();
    expect(
      screen.getByText("inventory.templates.editor.willCreateCount:1"),
    ).toBeInTheDocument();
    expect(previewTemplateRun).not.toHaveBeenCalled();
  });

  it("replaces the loading state with a persisted server preview", async () => {
    previewTemplateRun.mockResolvedValue({
      steps: [
        {
          step_key: "site",
          title: "Server site",
          resource_type: "site",
          payload: { name: "Paris" },
        },
      ],
    });

    render(<TemplatePreviewRail template={template} templateId="template-1" />);

    await waitFor(() => expect(previewTemplateRun).toHaveBeenCalledWith("template-1", {}), {
      timeout: 1_500,
    });
    expect(await screen.findByText("Server site")).toBeInTheDocument();
    expect(screen.getByText("Paris")).toBeInTheDocument();
  });
});
