import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { describe, expect, it, vi } from "vitest";
import SectionLinks from "./SectionLinks";

function Sections() {
  const location = useLocation();
  const navigate = useNavigate();
  const active = new URLSearchParams(location.search).get("section") ?? "profile";
  return (
    <>
      <SectionLinks label="Account sections" activeValue={active} items={[
        { value: "profile", label: "Profile", to: "/account?context=fixture#details" },
        { value: "tokens", label: "Tokens", badge: 0, to: "/account?context=fixture&section=tokens#details" },
      ]} />
      <output aria-label="Current URL">{location.pathname}{location.search}{location.hash}</output>
      <button onClick={() => void navigate(-1)}>Back</button>
      <button onClick={() => void navigate(1)}>Forward</button>
    </>
  );
}

describe("SectionLinks", () => {
  it("reveals selection within the strip without undoing manual scrolling on unrelated rerenders", () => {
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      return this.tagName === "NAV"
        ? { left: 0, right: 100, top: 0, bottom: 40, width: 100, height: 40, x: 0, y: 0, toJSON: () => ({}) }
        : { left: 200, right: 250, top: 0, bottom: 40, width: 50, height: 40, x: 200, y: 0, toJSON: () => ({}) };
    });
    const links = (activeValue: string, badge: number, includeActive = true) => <MemoryRouter><SectionLinks label="Entities" activeValue={activeValue} items={[
      { value: "sites", label: "Sites", to: "?entity=Sites", badge },
      ...(includeActive ? [{ value: "devices", label: "Devices", to: "?entity=Devices" }] : []),
    ]} /></MemoryRouter>;
    const { rerender } = render(links("devices", 1));
    const nav = screen.getByRole("navigation", { name: "Entities" });
    expect(nav.scrollLeft).toBe(150);
    const sites = screen.getByRole("link", { name: "Sites 1" });
    sites.focus();
    nav.scrollLeft = 500;
    fireEvent.scroll(nav);
    rerender(links("devices", 2));
    expect(nav.scrollLeft).toBe(500);
    expect(sites).toHaveFocus();
    rerender(links("sites", 2));
    expect(nav.scrollLeft).toBe(650);
    rerender(links("devices", 2, false));
    expect(nav.scrollLeft).toBe(650);
    rerender(links("devices", 2));
    expect(nav.scrollLeft).toBe(800);
    rect.mockRestore();
  });

  it("fits a late-growing active link until the user scrolls the strip", () => {
    let resize: ResizeObserverCallback = () => {};
    const disconnect = vi.fn();
    const originalObserver = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      constructor(callback: ResizeObserverCallback) { resize = callback; }
      observe() {}
      unobserve() {}
      disconnect() { disconnect(); }
    };
    let width = 15;
    const container: { nav?: HTMLElement } = {};
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const left = this.tagName === "NAV" ? 0 : 80 - (container.nav?.scrollLeft ?? 0);
      const size = this.tagName === "NAV" ? 100 : width;
      return { left, right: left + size, top: 0, bottom: 40, width: size, height: 40, x: left, y: 0, toJSON: () => ({}) };
    });
    const { unmount } = render(<MemoryRouter><SectionLinks label="Entities" activeValue="sites" items={[
      { value: "sites", label: "Sites", to: "?entity=Sites" },
    ]} /></MemoryRouter>);
    const nav = screen.getByRole("navigation", { name: "Entities" });
    container.nav = nav;
    expect(nav.scrollLeft).toBe(0);
    width = 30;
    resize([], {} as ResizeObserver);
    expect(nav.scrollLeft).toBe(10);
    fireEvent.scroll(nav);
    nav.scrollLeft = 30;
    fireEvent.scroll(nav);
    width = 70;
    resize([], {} as ResizeObserver);
    expect(nav.scrollLeft).toBe(30);
    unmount();
    expect(disconnect).toHaveBeenCalled();
    rect.mockRestore();
    globalThis.ResizeObserver = originalObserver;
  });

  it("reveals the active link when a preceding link grows", () => {
    let resize: ResizeObserverCallback = () => {};
    const observed = new Set<Element>();
    const originalObserver = globalThis.ResizeObserver;
    globalThis.ResizeObserver = class {
      constructor(callback: ResizeObserverCallback) { resize = callback; }
      observe(element: Element) { observed.add(element); }
      unobserve() {}
      disconnect() {}
    };
    let offset = 70;
    const container: { nav?: HTMLElement } = {};
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const left = this.getAttribute("aria-current") === "page" ? offset - (container.nav?.scrollLeft ?? 0) : 0;
      const width = this.tagName === "NAV" ? 100 : 25;
      return { left, right: left + width, top: 0, bottom: 40, width, height: 40, x: left, y: 0, toJSON: () => ({}) };
    });
    const { unmount } = render(<MemoryRouter><SectionLinks label="Entities" activeValue="devices" items={[
      { value: "sites", label: "Sites", to: "?entity=Sites" },
      { value: "devices", label: "Devices", to: "?entity=Devices" },
    ]} /></MemoryRouter>);
    const nav = screen.getByRole("navigation", { name: "Entities" });
    container.nav = nav;
    expect(nav.scrollLeft).toBe(0);
    expect(observed.has(screen.getByRole("link", { name: "Sites" }))).toBe(true);
    offset = 90;
    resize([], {} as ResizeObserver);
    expect(nav.scrollLeft).toBe(15);
    unmount();
    rect.mockRestore();
    globalThis.ResizeObserver = originalObserver;
  });

  it("exposes native section links, including a known zero badge, without panel roles", () => {
    render(<MemoryRouter initialEntries={["/account?context=fixture#details"]}><Sections /></MemoryRouter>);
    expect(screen.getByRole("navigation", { name: "Account sections" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Profile" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Tokens 0" })).toHaveAttribute("href", "/account?context=fixture&section=tokens#details");
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  });

  it("keeps query/hash context and follows back/forward rather than replacing history", () => {
    render(<MemoryRouter initialEntries={["/account?context=fixture#details"]}><Sections /></MemoryRouter>);
    fireEvent.click(screen.getByRole("link", { name: "Tokens 0" }));
    expect(screen.getByLabelText("Current URL")).toHaveTextContent("/account?context=fixture&section=tokens#details");
    expect(screen.getByRole("link", { name: "Tokens 0" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Profile" })).not.toHaveAttribute("aria-current");
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByRole("link", { name: "Profile" })).toHaveAttribute("aria-current", "page");
    fireEvent.click(screen.getByRole("button", { name: "Forward" }));
    expect(screen.getByRole("link", { name: "Tokens 0" })).toHaveAttribute("aria-current", "page");
  });

  it("notifies only same-view navigation, leaving modified native links alone", () => {
    const onNavigate = vi.fn();
    render(<MemoryRouter><SectionLinks label="Entities" activeValue="sites" onNavigate={onNavigate} items={[
      { value: "sites", label: "Sites", to: "?entity=Sites", badge: 4 },
      { value: "devices", label: "Devices", to: "?entity=Devices", badge: 0 },
    ]} /></MemoryRouter>);
    const link = screen.getByRole("link", { name: "Devices 0" });
    fireEvent.click(link, { ctrlKey: true });
    fireEvent.click(link, { metaKey: true });
    fireEvent.click(link, { shiftKey: true });
    fireEvent.click(link, { altKey: true });
    fireEvent.click(link, { button: 1 });
    expect(onNavigate).not.toHaveBeenCalled();
    expect(link).toHaveAttribute("href", "/?entity=Devices");
    fireEvent.click(link);
    expect(onNavigate).toHaveBeenCalledExactlyOnceWith("devices");
  });
});
