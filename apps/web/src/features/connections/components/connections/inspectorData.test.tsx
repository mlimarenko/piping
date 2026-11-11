import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { useHostNetworkInterfaces } from "./inspectorData";
import type * as InventoryApi from "@/features/inventory/api";

const calls = vi.hoisted(() => ({ interfaces: vi.fn(), addresses: vi.fn() }));
vi.mock("@/features/inventory/api", async (importOriginal) => ({
  ...await importOriginal<typeof InventoryApi>(),
  listDeviceInterfaces: calls.interfaces,
  listIpAddressesForDevice: calls.addresses,
  listVmInterfaces: calls.interfaces,
  listIpAddressesForVirtualMachine: calls.addresses,
}));

function Network({ id, enabled, kind }: { id: string; enabled: boolean; kind: "device" | "virtual_machine" }) {
  useHostNetworkInterfaces(kind, id, enabled);
  return null;
}

it.each(["device", "virtual_machine"] as const)("gates and aborts selected %s network requests", async (kind) => {
  calls.interfaces.mockReset();
  calls.addresses.mockReset();
  calls.interfaces.mockImplementation(() => new Promise(() => {}));
  calls.addresses.mockImplementation(() => new Promise(() => {}));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(<QueryClientProvider client={client}><Network id="A" enabled={false} kind={kind} /></QueryClientProvider>);
  expect(calls.interfaces).not.toHaveBeenCalled();
  expect(calls.addresses).not.toHaveBeenCalled();
  view.rerender(<QueryClientProvider client={client}><Network id="A" enabled kind={kind} /></QueryClientProvider>);
  await waitFor(() => expect(calls.interfaces).toHaveBeenCalledTimes(1));
  const interfaceSignal = calls.interfaces.mock.calls[0]![1] as AbortSignal;
  const addressSignal = calls.addresses.mock.calls[0]![1] as AbortSignal;
  view.rerender(<QueryClientProvider client={client}><Network id="B" enabled kind={kind} /></QueryClientProvider>);
  await waitFor(() => expect(interfaceSignal.aborted && addressSignal.aborted).toBe(true));
});
