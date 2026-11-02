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
