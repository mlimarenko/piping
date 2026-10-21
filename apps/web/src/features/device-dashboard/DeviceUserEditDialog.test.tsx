import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import "@/shared/i18n";
import DeviceUserEditDialog from "./DeviceUserEditDialog";
import type { DeviceUsersOutcome } from "./api";

const api = vi.hoisted(() => ({ updateDeviceUsers: vi.fn() }));
vi.mock("./api", () => api);
vi.mock("@/shared/lib/notifications", () => ({ notify: vi.fn() }));
beforeEach(() => { api.updateDeviceUsers.mockReset(); });

it.each(["success", "failure"])("locks the submitted account draft until %s", async result => {
  let resolve!: (value: DeviceUsersOutcome) => void;
  let reject!: (reason: Error) => void;
  const request = new Promise<DeviceUsersOutcome>((yes, no) => { resolve = yes; reject = no; });
  api.updateDeviceUsers.mockReturnValue(request);
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  render(<QueryClientProvider client={client}><DeviceUserEditDialog targetId="target-a"
    user={{ name: "operator", group: "Users", authorities: [] }} authorityOptions={["Read", "Write"]}
    onClose={() => undefined} onApplied={() => undefined} /></QueryClientProvider>);
  const group = screen.getByLabelText("Group");
  const password = screen.getByLabelText("Password");
  const picker = screen.getByRole("combobox", { name: "Authorities" });
  fireEvent.change(group, { target: { value: "Supervisors" } });
  fireEvent.click(picker);
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(api.updateDeviceUsers).toHaveBeenCalledTimes(1));
  expect(group).toBeDisabled();
  expect(password).toBeDisabled();
  expect(picker).toBeDisabled();
  fireEvent.change(group, { target: { value: "Administrators" } });
  fireEvent.change(password, { target: { value: "unsubmitted fixture" } });
  expect(group).toHaveValue("Supervisors");
  expect(password).toHaveValue("");
  expect(api.updateDeviceUsers).toHaveBeenCalledWith("target-a", [{ name: "operator", group: "Supervisors" }]);
  await act(async () => {
    if (result === "success") resolve({ succeeded: ["operator"], failed: [], all_succeeded: true });
    else reject(new Error("fixture failure"));
    await Promise.resolve();
  });
  if (result === "failure") {
    await waitFor(() => expect(group).toBeEnabled());
    expect(group).toHaveValue("Supervisors");
    fireEvent.change(group, { target: { value: "Administrators" } });
    expect(group).toHaveValue("Administrators");
  } else {
    await waitFor(() => expect(screen.getByRole("button", { name: "Close" })).toBeEnabled());
  }
});
