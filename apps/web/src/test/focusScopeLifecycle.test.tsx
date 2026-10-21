import { render, screen } from "@testing-library/react";
import { afterAll, expect, it, vi } from "vitest";
import { Dialog, DialogContent, DialogTitle } from "@/shared/ui-primitives/dialog";

const onCloseAutoFocus = vi.fn((event: Event) => {
  expect(event).toBeInstanceOf(window.Event);
});

it("completes Radix unmount autofocus in the rendering realm before suite teardown", () => {
  render(<Dialog open><DialogContent description="Lifecycle fixture" onCloseAutoFocus={onCloseAutoFocus}><DialogTitle>Lifecycle fixture</DialogTitle><button type="button">Focus target</button></DialogContent></Dialog>);
  expect(screen.getByRole("dialog")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Focus target" })).toHaveFocus();
});

it("completes the same unmount lifecycle under test-owned fake timers", () => {
  vi.useFakeTimers();
  render(<Dialog open><DialogContent description="Fake-timer lifecycle fixture" onCloseAutoFocus={onCloseAutoFocus}><DialogTitle>Fake-timer lifecycle fixture</DialogTitle><button type="button">Focus target</button></DialogContent></Dialog>);
  expect(screen.getByRole("button", { name: "Focus target" })).toHaveFocus();
});

afterAll(() => {
  vi.useRealTimers();
  // The shared teardown must execute the real deferred event, not discard it.
  expect(onCloseAutoFocus).toHaveBeenCalledTimes(2);
});
