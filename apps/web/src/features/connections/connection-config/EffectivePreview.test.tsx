import { fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import i18n from "@/shared/i18n";
import EffectivePreview from "./EffectivePreview";
import type * as ConfigApi from "./api";

const api = vi.hoisted(() => ({ getEffectiveConfig: vi.fn(), listRecords: vi.fn(), listResources: vi.fn() }));
vi.mock("./api", async original => ({ ...await original<typeof ConfigApi>(), getEffectiveConfig: api.getEffectiveConfig, listRecords: api.listRecords }));
vi.mock("@/features/inventory/api", () => ({ listResources: api.listResources }));

beforeEach(async () => {
  await i18n.changeLanguage("en");
  api.listResources.mockImplementation((kind: string) => Promise.resolve({ items: kind === "Devices" ? [{ id: "target", name: "Target" }] : [] }));
  api.listRecords.mockResolvedValue([{ id: "a", name: "Alpha", protocol: "ssh", enabled: true }, { id: "b", name: "Beta", protocol: "ssh", enabled: true }]);
  api.getEffectiveConfig.mockResolvedValue({ params: {
    host: "destination.example", credentials: { username: "private-user", password: "private-password", private_key: "private-key" },
    jump: [{ connconf_record_id: "b", credentials: { password: "hop-password" } }, { connconf_record_id: "missing" }, { connconf_record_id: "a" }],
    extra: [{ credentials: { password: "nested-password" } }],
  }, attribution: {}, resolution_chain: [] });
});

it("previews ordered hop names, unavailable references and the destination without credentials", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><EffectivePreview /></QueryClientProvider>);
  fireEvent.click(screen.getByRole("combobox", { name: "Device" }));
  fireEvent.click(await screen.findByRole("option", { name: "Target" }));
  const title = await screen.findByText("Effective SSH route");
  const section = title.closest('[data-slot="form-section"]');
  expect(section).not.toBeNull();
  const route = within(section as HTMLElement);
  await route.findByText("1. Beta");
  expect(route.getAllByRole("listitem").map(item => item.textContent)).toEqual([
    "1. Beta", "2. Unavailable: missing", "3. Alpha", "4. Destination: destination.example",
  ]);
  for (const secret of ["private-user", "private-password", "private-key", "hop-password", "nested-password"]) {
    expect(screen.queryByText(new RegExp(secret))).not.toBeInTheDocument();
  }
  expect(api.getEffectiveConfig).toHaveBeenCalledWith("target", "ssh");
});
