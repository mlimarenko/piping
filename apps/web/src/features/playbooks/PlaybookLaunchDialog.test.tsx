import { fireEvent, render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import type { ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";
import "@/shared/i18n";
import PlaybookLaunchDialog from "./PlaybookLaunchDialog";
import type { components } from "@/shared/api/schema.gen";
import type { AutomationSecret } from "@/features/automation-secrets/api";
import type { PlaybookTargetPreviewItem, PlaybookTargetSetPreview } from "./api";
import type { LaunchResource } from "./launchReadiness";

const job: components["schemas"]["JobTemplate"] = { id: "job-1", name: "Deploy", project_id: "project-1", playbook_paths: ["site.yml"], workdir: ".", execution_environment_id: "env", inventory: { kind: "managed", target_set_id: "set" }, revision: { kind: "current" }, options: { argv: [], env: {}, extra_vars: {}, secret_refs: {}, timeout_seconds: 60 } };
const project: components["schemas"]["Project"] = { id: "project-1", name: "Operations", source: { kind: "editable" }, current_revision_id: "abc123", created_at: "", updated_at: "" };

function host(over: Partial<PlaybookTargetPreviewItem> = {}): PlaybookTargetPreviewItem {
  return { name: "Alpha", address: "10.0.0.1", address_bound: true, source_id: "source-1", source_kind: "device", target_id: "target-1", target_kind: "connection_target", ...over };
}

function preview(over: Partial<PlaybookTargetSetPreview> = {}): PlaybookTargetSetPreview {
  return {
    items: [host(), host({ name: "Beta", target_id: "target-2", address: "10.0.0.2" })],
    total: 2,
    limit: 500,
    truncated: false,
    target_set: { id: "set", name: "Edge routers", mode: "dynamic", entity_kind: "connection_target", selector: {}, created_at: "", updated_at: "" },
    ...over,
  };
}

const secret = (over: Partial<AutomationSecret> = {}): AutomationSecret => ({ id: "secret-1", key: "API_TOKEN", has_value: true, is_masked: true, usage_count: 1, created_at: "", ...over });

const readyPreview = (data: PlaybookTargetSetPreview = preview()): LaunchResource<PlaybookTargetSetPreview> => ({ status: "ready", data });
const readySecrets = (data: AutomationSecret[] = []): LaunchResource<AutomationSecret[]> => ({ status: "ready", data });

type Props = ComponentProps<typeof PlaybookLaunchDialog>;

function renderDialog(over: Partial<Props> = {}) {
  const props: Props = { open: true, intent: { jobTemplateId: job.id }, onOpenChange: vi.fn(), onConfirm: vi.fn(), jobTemplates: [job], projects: [project], onSelectedJobTemplateIdChange: vi.fn(), canLaunch: true, preview: readyPreview(), secrets: readySecrets(), ...over };
  return render(<MemoryRouter><PlaybookLaunchDialog {...props} /></MemoryRouter>);
}

const launchButton = () => screen.getByRole("button", { name: /^launch$/i });

describe("job launch", () => {
  const parameterizedJob = { ...job, vars_schema: { kind: "inline" as const, schema: { type: "object", properties: {
    required_name: { type: "string", title: "Required name" },
    count: { type: "integer", title: "Count", default: 5 },
    enabled: { type: "boolean", title: "Enabled", default: false },
    optional_name: { type: "string", title: "Optional name" },
  }, required: ["required_name", "count", "enabled"] } } };

  it("shows all declared fields in normal launch and sends only run overrides", () => {
    const confirm = vi.fn();
    renderDialog({ jobTemplates: [parameterizedJob], onConfirm: confirm });
    expect(screen.getByLabelText("Optional name")).toBeInTheDocument();
    expect(launchButton()).toBeDisabled();
    fireEvent.click(within(screen.getByLabelText("Required name *").parentElement!).getByRole("button", { name: "Set empty string" }));
    fireEvent.change(screen.getByLabelText("Count *"), { target: { value: "0" } });
    expect(launchButton()).toBeEnabled();
    fireEvent.click(launchButton());
    expect(confirm).toHaveBeenCalledWith({ jobTemplateId: job.id, extraVars: { required_name: "", count: 0 } });
  });

  it("asks only for required missing values in quick launch", () => {
    const confirm = vi.fn();
    renderDialog({ quick: true, jobTemplates: [parameterizedJob], onConfirm: confirm });
    expect(screen.queryByLabelText("Optional name")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Count *")).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Required name *"), { target: { value: "quick" } });
    fireEvent.click(launchButton());
    expect(confirm).toHaveBeenCalledWith({ jobTemplateId: job.id, extraVars: { required_name: "quick" } });
  });

  it("does not launch stale valid values while a raw override is invalid", () => {
    renderDialog({ jobTemplates: [parameterizedJob] });
    fireEvent.click(screen.getByRole("button", { name: "YAML" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Parameters YAML" }), { target: { value: 'required_name: valid' } });
    expect(launchButton()).toBeEnabled();
    fireEvent.change(screen.getByRole("textbox", { name: "Parameters YAML" }), { target: { value: 'required_name: [' } });
    expect(launchButton()).toBeDisabled();
  });

  it("fails closed until the pinned project schema is loaded", () => {
    const fileJob = { ...job, vars_schema: { kind: "project_file" as const, path: "parameters.json" } };
    const confirm = vi.fn();
    renderDialog({ jobTemplates: [fileJob], varsSchema: { status: "failed" }, onConfirm: confirm });
    expect(launchButton()).toBeDisabled();
    expect(screen.getByRole("status")).toHaveTextContent(/schema could not be loaded/i);
    fireEvent.click(launchButton());
    expect(confirm).not.toHaveBeenCalled();
  });

  it("opens launch help separately without queuing a run or closing the draft", () => {
    const confirm = vi.fn();
    const close = vi.fn();
    renderDialog({ onConfirm: confirm, onOpenChange: close });
    const help = screen.getByRole("link", { name: /documentation/i });
    expect(help).toHaveAttribute("href", "https://piping.space/docs/jobs-and-runs#launch-checks");
    expect(help).toHaveAttribute("target", "_blank");
    expect(help).toHaveAttribute("rel", "noopener noreferrer");
    fireEvent.click(help);
    expect(confirm).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("shows the saved target set, its mode, count, first hosts and the full-preview link", () => {
    const confirm = vi.fn();
    renderDialog({ onConfirm: confirm });
    expect(screen.getByText("Operations")).toBeInTheDocument();
    expect(screen.getByText("abc123")).toBeInTheDocument();
    expect(screen.getByText(/saved job configuration/i)).toBeInTheDocument();
    expect(screen.getByText("Edge routers")).toBeInTheDocument();
    expect(screen.getByText("Dynamic selector")).toBeInTheDocument();
    expect(screen.getByText("Hosts").parentElement).toHaveTextContent("2");
    const sample = screen.getByText("Alpha").closest("ol");
    expect(sample).not.toBeNull();
    expect(within(sample as HTMLElement).getByText("Beta")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /open full preview/i })).toHaveAttribute("href", "/playbooks/settings/target-sets?targetSet=set");
    fireEvent.click(launchButton());
    expect(confirm).toHaveBeenCalledWith({ jobTemplateId: "job-1" });
  });

  it("reports a set that resolves to no hosts and blocks the launch", () => {
    renderDialog({ preview: readyPreview(preview({ items: [], total: 0 })) });
    expect(screen.getByText("The saved target set resolves to no hosts.")).toBeInTheDocument();
    expect(launchButton()).toBeDisabled();
  });

  it("blocks a composition above the agreed UI limit and names both numbers", () => {
    renderDialog({ preview: readyPreview(preview({ total: 501 })) });
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("501");
    expect(alert).toHaveTextContent("500");
    expect(launchButton()).toBeDisabled();
  });

  it("blocks hosts without a bound address", () => {
    renderDialog({ preview: readyPreview(preview({ items: [host(), host({ name: "Beta", address: null, address_bound: false })] })) });
    expect(screen.getByRole("alert")).toHaveTextContent("1 hosts have no bound address.");
    expect(screen.getByText("No bound address")).toBeInTheDocument();
    expect(launchButton()).toBeDisabled();
  });

  it("cannot launch without explicit permission or a resolved project", () => {
    const props = { open: true, intent: { jobTemplateId: job.id }, onOpenChange: vi.fn(), onConfirm: vi.fn(), jobTemplates: [job], onSelectedJobTemplateIdChange: vi.fn(), preview: readyPreview(), secrets: readySecrets() };
    const { rerender } = render(<MemoryRouter><PlaybookLaunchDialog {...props} projects={[project]} canLaunch={false} /></MemoryRouter>);
    expect(screen.getByRole("alert")).toHaveTextContent("execute permission");
    expect(launchButton()).toBeDisabled();
    rerender(<MemoryRouter><PlaybookLaunchDialog {...props} projects={[]} canLaunch /></MemoryRouter>);
    expect(launchButton()).toBeDisabled();
  });

  it("cannot launch a current-selection job before its project has a saved revision", () => {
    renderDialog({ projects: [{ ...project, current_revision_id: null }] });
    expect(launchButton()).toBeDisabled();
  });

  it("blocks a referenced secret that is missing or has no value", () => {
    const withSecret = { ...job, options: { ...job.options, secret_refs: { API_TOKEN: "secret-1" } } };
    const props = { open: true, intent: { jobTemplateId: job.id }, onOpenChange: vi.fn(), onConfirm: vi.fn(), projects: [project], onSelectedJobTemplateIdChange: vi.fn(), canLaunch: true, preview: readyPreview() };
    const { rerender } = render(<MemoryRouter><PlaybookLaunchDialog {...props} jobTemplates={[withSecret]} secrets={readySecrets([])} /></MemoryRouter>);
    expect(screen.getByRole("alert")).toHaveTextContent("1 referenced secrets are missing or have no value.");
    expect(launchButton()).toBeDisabled();
    rerender(<MemoryRouter><PlaybookLaunchDialog {...props} jobTemplates={[withSecret]} secrets={readySecrets([secret({ has_value: false })])} /></MemoryRouter>);
    expect(launchButton()).toBeDisabled();
    rerender(<MemoryRouter><PlaybookLaunchDialog {...props} jobTemplates={[withSecret]} secrets={readySecrets([secret()])} /></MemoryRouter>);
    expect(launchButton()).toBeEnabled();
  });

  it("tells a failed composition load apart from an empty one and retries", () => {
    const retry = vi.fn();
    renderDialog({ preview: { status: "failed" }, onRetryComposition: retry });
    expect(screen.getByRole("alert")).toHaveTextContent("could not be loaded");
    expect(screen.queryByText("The saved target set resolves to no hosts.")).not.toBeInTheDocument();
    expect(screen.queryByText("Hosts")).not.toBeInTheDocument();
    expect(launchButton()).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /^retry$/i }));
    expect(retry).toHaveBeenCalledOnce();
  });

  it("blocks while the composition is still loading", () => {
    renderDialog({ preview: { status: "loading" } });
    expect(screen.getByRole("status")).toHaveTextContent("Checking the target set composition.");
    expect(launchButton()).toBeDisabled();
  });

  it("blocks when the job's secrets cannot be loaded", () => {
    const withSecret = { ...job, options: { ...job.options, secret_refs: { API_TOKEN: "secret-1" } } };
    renderDialog({ jobTemplates: [withSecret], secrets: { status: "failed" } });
    expect(screen.getByRole("alert")).toHaveTextContent("The job's secrets could not be loaded");
    expect(launchButton()).toBeDisabled();
  });

  it("explains the managed inventory constraint and blocks an empty selection", () => {
    const confirm = vi.fn();
    const props = { open: true, intent: { jobTemplateId: job.id }, onOpenChange: vi.fn(), onConfirm: confirm, jobTemplates: [job], projects: [project], onSelectedJobTemplateIdChange: vi.fn(), canLaunch: true, secrets: readySecrets() };
    const hosts = [{ id: "host-1", name: "Alpha", sourceKind: "device" as const, sourceId: "device-1" }, { id: "host-2", name: "Beta", sourceKind: "virtual_machine" as const, sourceId: "vm-1" }];
    const { rerender } = render(<MemoryRouter><PlaybookLaunchDialog {...props} fixedTargets={hosts} /></MemoryRouter>);
    expect(screen.getByText(/must belong to the job's saved managed inventory/i)).toBeInTheDocument();
    expect(screen.getByText("Selected hosts: 2")).toBeInTheDocument();
    expect(screen.getByText("Alpha")).toBeInTheDocument();
    expect(screen.getByText("Beta")).toBeInTheDocument();
    fireEvent.click(launchButton());
    expect(confirm).toHaveBeenCalledWith({ jobTemplateId: job.id });
    rerender(<MemoryRouter><PlaybookLaunchDialog {...props} fixedTargets={[]} /></MemoryRouter>);
    expect(screen.getByRole("alert")).toHaveTextContent("Select at least one host");
    expect(launchButton()).toBeDisabled();
    rerender(<MemoryRouter><PlaybookLaunchDialog {...props} /></MemoryRouter>);
    expect(launchButton()).toBeDisabled();
  });

  it("rejects fixed host selection for native inventory", () => {
    const confirm = vi.fn();
    renderDialog({ jobTemplates: [{ ...job, inventory: { kind: "native", sources: [] } }], onConfirm: confirm, fixedTargets: [{ id: "host-1", name: "Alpha", sourceKind: "device", sourceId: "device-1" }] });
    expect(screen.getByText(/native inventory does not support host selection/i)).toBeInTheDocument();
    expect(launchButton()).toBeDisabled();
    expect(confirm).not.toHaveBeenCalled();
  });

  it("shows the native inventory instead of a resolved host list", () => {
    renderDialog({ jobTemplates: [{ ...job, inventory: { kind: "native", sources: [] } }], preview: undefined });
    expect(screen.getByText(/runs against its native inventory/i)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /open full preview/i })).not.toBeInTheDocument();
    expect(launchButton()).toBeEnabled();
  });

  it("retains the selected hosts and shows launch rejection without a fallback action", () => {
    const confirm = vi.fn();
    renderDialog({ onConfirm: confirm, fixedTargets: [{ id: "host-1", name: "Alpha", sourceKind: "device", sourceId: "device-1" }], launchError: "Launch rejected: selected hosts must belong to the saved managed inventory." });
    expect(screen.getByRole("alert")).toHaveTextContent("Launch rejected");
    expect(screen.getByText("Alpha")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /saved targets|all hosts/i })).not.toBeInTheDocument();
    expect(confirm).not.toHaveBeenCalled();
  });

  it("blocks launch on a reference error and retries inside the dialog", () => {
    const retry = vi.fn();
    const confirm = vi.fn();
    const props = { open: true, intent: { jobTemplateId: job.id }, onOpenChange: vi.fn(), onConfirm: confirm, jobTemplates: [job], projects: [project], onSelectedJobTemplateIdChange: vi.fn(), canLaunch: true, preview: readyPreview(), secrets: readySecrets() };
    const { rerender } = render(<MemoryRouter><PlaybookLaunchDialog {...props} referenceError="Reference data unavailable" onRetry={retry} /></MemoryRouter>);
    expect(screen.getByRole("alert")).toHaveTextContent("Reference data unavailable");
    expect(launchButton()).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: /^retry$/i }));
    expect(retry).toHaveBeenCalledOnce();
    expect(confirm).not.toHaveBeenCalled();
    rerender(<MemoryRouter><PlaybookLaunchDialog {...props} /></MemoryRouter>);
    expect(launchButton()).toBeEnabled();
  });
});
