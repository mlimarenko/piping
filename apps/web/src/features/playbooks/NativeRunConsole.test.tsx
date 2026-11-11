import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type * as ReactI18next from "react-i18next";
import { NativeRunConsole } from "./NativeRunConsole";
import { ApiError } from "@/shared/api/client";

const mocks = vi.hoisted(() => ({ write: vi.fn(), fetch: vi.fn(), copy: vi.fn() }));
vi.mock("./api", () => ({ getRunGroupConsole: mocks.fetch }));
vi.mock("react-i18next", async (original) => ({ ...await original<typeof ReactI18next>(), useTranslation: () => ({ t: (_key: string, options: { defaultValue: string }) => options.defaultValue }) }));
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/xterm", () => ({ Terminal: class {
  buffer = { active: { length: 0, viewportY: 0 } };
  loadAddon() {} open() {} dispose() {} scrollToBottom() {} scrollToLine() {}
  write(text: string, callback?: () => void) { mocks.write(text); callback?.(); }
} }));
const id = "00000000-0000-4000-8000-000000000198";
const response = (sequence: number, bytes: number[], complete = false) => ({ dispatch_id: id, dispatch_attempt: 1, chunks: [{ sequence, stream: "stdout", bytes }], next_sequence: sequence + 1, complete });
afterEach(() => { vi.clearAllMocks(); vi.useRealTimers(); });

it("shows errors and retries at the retained cursor", async () => {
  mocks.fetch.mockResolvedValueOnce(response(0, [65])).mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce(response(1, [66], true));
  render(<NativeRunConsole groupId="group" label="Output" />);
  await waitFor(() => expect(mocks.write).toHaveBeenCalledWith("A"));
  await screen.findByRole("alert", {}, { timeout: 4000 });
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  await waitFor(() => expect(mocks.write).toHaveBeenCalledWith("B"));
  expect(mocks.fetch.mock.calls[2]![1]).toMatchObject({ fromSequence: 1, dispatchId: id });
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it("aborts on unmount and ignores late responses", async () => {
  let resolve!: (value: unknown) => void;
  mocks.fetch.mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
  const view = render(<NativeRunConsole groupId="group" label="Output" />);
  const signal = mocks.fetch.mock.calls[0]![1].signal as AbortSignal;
  view.unmount(); expect(signal.aborted).toBe(true);
  resolve(response(0, [65], true)); await Promise.resolve();
  expect(mocks.write).not.toHaveBeenCalled();
});

it("aborts an old group request before consuming the replacement group", async () => {
  let resolve!: (value: unknown) => void;
  mocks.fetch.mockImplementationOnce(() => new Promise((done) => { resolve = done; })).mockResolvedValueOnce(response(0, [66], true));
  const view = render(<NativeRunConsole groupId="old" label="Output" />);
  const signal = mocks.fetch.mock.calls[0]![1].signal as AbortSignal;
  view.rerender(<NativeRunConsole groupId="new" label="Output" />);
  expect(signal.aborted).toBe(true);
  resolve(response(0, [65], true));
  await waitFor(() => expect(mocks.write).toHaveBeenCalledWith("B"));
  expect(mocks.write).not.toHaveBeenCalledWith("A");
});

it("copies durable bytes, not the terminal screen", async () => {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: mocks.copy.mockResolvedValue(undefined) } });
  mocks.fetch.mockResolvedValue(response(0, [27, 91, 51, 49, 109, 65, 13, 66, 255], true));
  render(<NativeRunConsole groupId="group" label="Output" />);
  await waitFor(() => expect(mocks.write).toHaveBeenCalled());
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  await waitFor(() => expect(mocks.copy).toHaveBeenCalledWith("\x1b[31mA\rB�"));
  expect(mocks.fetch).toHaveBeenCalledTimes(2);
});

it("shows dispatch conflicts without retrying automatically", async () => {
  mocks.fetch.mockRejectedValueOnce(new ApiError({ status: 409, message: "conflict" }));
  render(<NativeRunConsole groupId="group" label="Output" />);
  expect(await screen.findByRole("alert")).toHaveTextContent("Console dispatch changed");
  expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
  expect(mocks.fetch).toHaveBeenCalledTimes(1);
});

it("disables export while queued but allows a sealed empty dispatch", async () => {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: mocks.copy.mockResolvedValue(undefined) } });
  mocks.fetch.mockResolvedValueOnce({ dispatch_id: null, dispatch_attempt: 0, chunks: [], next_sequence: 0, complete: false }).mockResolvedValueOnce({ dispatch_id: id, dispatch_attempt: 1, chunks: [], next_sequence: 0, complete: true });
  render(<NativeRunConsole groupId="group" label="Output" />);
  expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
  await waitFor(() => expect(screen.getByRole("button", { name: "Copy" })).toBeEnabled(), { timeout: 4000 });
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  await waitFor(() => expect(mocks.copy).toHaveBeenCalledWith(""));
  expect(screen.getByText("Complete output exported.")).toBeInTheDocument();
});

it("disposes an in-flight export and resets the replacement group session", async () => {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: mocks.copy.mockResolvedValue(undefined) } });
  let resolve!: (value: unknown) => void;
  mocks.fetch.mockResolvedValueOnce(response(0, [65], true)).mockImplementationOnce(() => new Promise((done) => { resolve = done; })).mockResolvedValueOnce(response(0, [66], true));
  const view = render(<NativeRunConsole groupId="old" label="Output" />);
  await waitFor(() => expect(screen.getByRole("button", { name: "Copy" })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: "Pause follow" }));
  fireEvent.change(screen.getByLabelText("Search output"), { target: { value: "old" } });
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  const signal = mocks.fetch.mock.calls[1]![1].signal as AbortSignal;
  view.rerender(<NativeRunConsole groupId="new" label="Output" />);
  expect(signal.aborted).toBe(true);
  expect(screen.getByLabelText("Search output")).toHaveValue("");
  expect(screen.getByRole("button", { name: "Pause follow" })).toHaveAttribute("aria-pressed", "true");
  resolve(response(0, [65], true));
  await waitFor(() => expect(screen.getByRole("button", { name: "Copy" })).toBeEnabled());
  expect(mocks.copy).not.toHaveBeenCalled();
});

it("keeps polling an empty page until output arrives, independent of status", async () => {
  mocks.fetch.mockResolvedValueOnce({ dispatch_id: id, dispatch_attempt: 1, chunks: [], next_sequence: 0, complete: false }).mockResolvedValueOnce(response(0, [65], true));
  render(<NativeRunConsole groupId="group" label="Output" />);
  await waitFor(() => expect(mocks.write).toHaveBeenCalledWith("A"), { timeout: 4000 });
  expect(mocks.fetch.mock.calls[1]![1]).toMatchObject({ fromSequence: 0, dispatchId: id });
});

it("drains nonempty pages promptly without overlapping requests", async () => {
  let resolve!: (value: unknown) => void;
  mocks.fetch.mockResolvedValueOnce(response(0, [65])).mockImplementationOnce(() => new Promise((done) => { resolve = done; })).mockResolvedValueOnce(response(2, [67], true));
  render(<NativeRunConsole groupId="group" label="Output" />);
  await waitFor(() => expect(mocks.fetch).toHaveBeenCalledTimes(2), { timeout: 1000 });
  expect(mocks.fetch.mock.calls[1]![1]).toMatchObject({ fromSequence: 1, dispatchId: id });
  expect(mocks.fetch).toHaveBeenCalledTimes(2);
  resolve(response(1, [66]));
  await waitFor(() => expect(mocks.write).toHaveBeenCalledWith("C"), { timeout: 1000 });
  expect(mocks.fetch.mock.calls[2]![1]).toMatchObject({ fromSequence: 2, dispatchId: id });
});

it("preserves invalid bytes in the downloaded blob", async () => {
  const create = vi.fn((_blob: Blob) => "blob:raw");
  vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: create, revokeObjectURL: vi.fn() }));
  const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
  mocks.fetch.mockResolvedValue(response(0, [255, 13, 27], true));
  render(<NativeRunConsole groupId="group" label="Output" />);
  await waitFor(() => expect(mocks.write).toHaveBeenCalled());
  fireEvent.click(screen.getByRole("button", { name: "Download raw" }));
  await waitFor(() => expect(create).toHaveBeenCalled());
  const blob = create.mock.calls[0]![0];
  expect(blob.size).toBe(3);
  const buffer = await new Promise<ArrayBuffer>((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(reader.result as ArrayBuffer); reader.onerror = reject; reader.readAsArrayBuffer(blob); });
  expect(Array.from(new Uint8Array(buffer))).toEqual([255, 13, 27]);
  click.mockRestore(); vi.unstubAllGlobals();
});

it("exports more than the old 4 MiB retention as durable bytes", async () => {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: mocks.copy.mockResolvedValue(undefined) } });
  const bytes = Array(65536).fill(65) as number[];
  const last = response(64, [66], true);
  mocks.fetch.mockImplementation((_group: string, options: { fromSequence: number }) => Promise.resolve(options.fromSequence === 64 ? last : { dispatch_id: id, dispatch_attempt: 1, chunks: Array.from({ length: 4 }, (_, offset) => ({ sequence: options.fromSequence + offset, stream: "stdout", bytes })), next_sequence: options.fromSequence + 4, complete: false }));
  render(<NativeRunConsole groupId="group" label="Output" />);
  await waitFor(() => expect(mocks.write).toHaveBeenCalledWith("B"), { timeout: 6000 });
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  await waitFor(() => expect(mocks.copy).toHaveBeenCalled(), { timeout: 6000 });
  const text = mocks.copy.mock.calls[0]![0] as string;
  expect(text.length).toBe(4 * 1024 * 1024 + 1);
  expect(text.endsWith("B")).toBe(true);
}, 15000);
