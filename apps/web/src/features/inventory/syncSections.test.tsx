import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router";
import { describe, expect, it } from "vitest";
import i18n from "@/shared/i18n";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import { syncSectionLinks } from "./syncSections";

function Sections({ counts, providerId }: { counts?: { providers?: number; mappings?: number }; providerId?: string }) {
  const location = useLocation();
  const active = location.pathname.endsWith("mappings") ? "mappings" : "providers";
  return (
    <>
      <SectionLinks label={i18n.t("inventory.sync.sectionsLabel")} activeValue={active}
        items={syncSectionLinks(i18n.t.bind(i18n), counts ?? {}, location, active, providerId)} />
      <output aria-label="Current URL">{location.pathname}{location.search}{location.hash}</output>
    </>
  );
}

describe("sync section links", () => {
  it("keeps the provider scope when reselecting the active mappings section", () => {
    render(<MemoryRouter initialEntries={["/inventory/sync/provider-1/mappings?context=fixture#selection"]}>
      <Sections counts={{}} />
    </MemoryRouter>);
    const mappings = screen.getByRole("link", { name: i18n.t("inventory.sync.mappingsTab") });
    expect(mappings).toHaveAttribute("href", "/inventory/sync/provider-1/mappings?context=fixture#selection");
    fireEvent.click(mappings);
    expect(screen.getByLabelText("Current URL")).toHaveTextContent("/inventory/sync/provider-1/mappings?context=fixture#selection");
  });

  it("preserves provider/mapping counts and destination context as native links", () => {
    render(<MemoryRouter initialEntries={["/inventory/sync?context=fixture#selection"]}>
      <Sections counts={{ providers: 3, mappings: 7 }} />
    </MemoryRouter>);
    const providers = screen.getByRole("link", { name: `${i18n.t("inventory.sync.providersTab")} (3)` });
    expect(screen.getByRole("navigation", { name: i18n.t("inventory.sync.sectionsLabel") })).toBeInTheDocument();
    const mappings = screen.getByRole("link", { name: `${i18n.t("inventory.sync.mappingsTab")} (7)` });
    expect(providers).toHaveAttribute("aria-current", "page");
    expect(mappings).toHaveAttribute("href", "/inventory/sync/mappings?context=fixture#selection");
    fireEvent.click(mappings);
    expect(screen.getByLabelText("Current URL")).toHaveTextContent("/inventory/sync/mappings?context=fixture#selection");
    expect(mappings).toHaveAttribute("aria-current", "page");
  });

  it("does not invent unknown totals and exposes a known zero", () => {
    render(<MemoryRouter initialEntries={["/inventory/sync/mappings"]}>
      <Sections counts={{ mappings: 0 }} />
    </MemoryRouter>);
    expect(screen.getByRole("link", { name: i18n.t("inventory.sync.providersTab") })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: `${i18n.t("inventory.sync.mappingsTab")} (0)` })).toHaveAttribute("aria-current", "page");
  });

  it("scopes the mappings link to the selected provider", () => {
    render(<MemoryRouter initialEntries={["/inventory/sync?provider=provider-1"]}>
      <Sections providerId="provider-1" />
    </MemoryRouter>);
    const mappings = screen.getByRole("link", { name: i18n.t("inventory.sync.mappingsTab") });
    expect(mappings).toHaveAttribute("href", "/inventory/sync/provider-1/mappings");
  });

  it("carries the selected provider back to the providers list from a scoped mappings list", () => {
    render(<MemoryRouter initialEntries={["/inventory/sync/provider-1/mappings"]}>
      <Sections providerId="provider-1" />
    </MemoryRouter>);
    const providers = screen.getByRole("link", { name: i18n.t("inventory.sync.providersTab") });
    expect(providers).toHaveAttribute("href", "/inventory/sync?provider=provider-1");
  });
});
