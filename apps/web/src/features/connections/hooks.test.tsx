import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import {
  connectionLaunchOptionsQueryKey,
  prefetchConnectionLaunchOptions,
  useConnectionLaunchOptions,
} from "./hooks";

const fetchOptions = vi.hoisted(() => vi.fn());
vi.mock("@/features/connections/api", () => ({ getConnectionLaunchOptions: fetchOptions }));

function Observer({ id, enabled = true }: { id: string; enabled?: boolean }) {
  const query = useConnectionLaunchOptions(id, { enabled });
  return <span>{id}:{query.data?.hostId ?? "pending"}</span>;
}

it("aborts an abandoned target even while an uninterested row remains mounted", async () => {
  const signals = new Map<string, AbortSignal>();
  fetchOptions.mockImplementation((id: string, signal: AbortSignal) => {
    signals.set(id, signal);
    return new Promise(() => {});
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(<QueryClientProvider client={client}><Observer id="A" enabled={false} /><Observer id="A" /></QueryClientProvider>);
  await waitFor(() => expect(signals.has("A")).toBe(true));
  view.rerender(<QueryClientProvider client={client}><Observer id="A" enabled={false} /><Observer id="B" /></QueryClientProvider>);
  await waitFor(() => expect(signals.get("A")?.aborted).toBe(true));
  expect(screen.getByText("B:pending")).toBeInTheDocument();
  expect(signals.get("B")?.aborted).toBe(false);
});

it("deduplicates an actively shared target without cancelling an interested consumer", async () => {
  fetchOptions.mockReset();
  let signal: AbortSignal | undefined;
  fetchOptions.mockImplementation((id: string, current: AbortSignal) => {
    if (id === "A") signal = current;
    return new Promise(() => {});
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(<QueryClientProvider client={client}><Observer id="A" /><Observer id="A" /></QueryClientProvider>);
  await waitFor(() => expect(fetchOptions).toHaveBeenCalledTimes(1));
  view.rerender(<QueryClientProvider client={client}><Observer id="A" /><Observer id="B" /></QueryClientProvider>);
  await waitFor(() => expect(fetchOptions).toHaveBeenCalledTimes(2));
  expect(signal?.aborted).toBe(false);
});

it("shares a selection prefetch with the inspector observer instead of fetching twice", async () => {
  fetchOptions.mockReset();
  let resolve!: (value: { hostId: string; protocols: [] }) => void;
  fetchOptions.mockImplementation(() => new Promise((done) => { resolve = done; }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });

  const prefetched = prefetchConnectionLaunchOptions(client, "A");
  const view = render(<QueryClientProvider client={client}><Observer id="A" /></QueryClientProvider>);
  await waitFor(() => expect(fetchOptions).toHaveBeenCalledTimes(1));

  await act(async () => {
    resolve({ hostId: "A", protocols: [] });
    await prefetched;
  });

  expect(await screen.findByText("A:A")).toBeInTheDocument();
  expect(fetchOptions).toHaveBeenCalledTimes(1);
  expect(client.getQueryData(connectionLaunchOptionsQueryKey("A"))).toEqual({ hostId: "A", protocols: [] });
  view.unmount();
});
