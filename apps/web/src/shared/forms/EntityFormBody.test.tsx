import { useState } from "react";
import { fireEvent, screen } from "@testing-library/react";
import { render } from "@/test/pageChrome";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AllPermissions } from "@/test/permissions";
import i18n from "@/shared/i18n";
import EntityFormBody from "./EntityFormBody";
import { buildEntityDraft, draftToResourceUpdate, validateEntityDraft } from "./entityForm";

function NetworkingForm() {
  const [draft, setDraft] = useState(() => buildEntityDraft("Interfaces", {
    id: "i", name: "eth0", type: "interface", deviceId: "d", mtu: "1500",
    mode: "q-in-q", speed: "0", txPower: "-40", rfChannelFrequency: "5180.125",
    cablePositions: "1, 1024",
  }));
  return <EntityFormBody entity="Interfaces" draft={draft}
    onField={(key, value) => setDraft((current) => ({ ...current, [key]: value }))}
    loadOptions={() => []} onRequestCreate={() => undefined} />;
}

beforeEach(async () => { await i18n.changeLanguage("en"); });

describe("Interface networking controls", () => {
  it("renders seeded networking values, numeric limits and an accessible enum clear action", () => {
    render(<AllPermissions><NetworkingForm /></AllPermissions>);
    expect(screen.getByRole("spinbutton", { name: "MTU" })).toHaveValue(1500);
    expect(screen.getByRole("spinbutton", { name: "MTU" })).toHaveAttribute("max", "65536");
    // Speed is a decimal-string input so values above Number.MAX_SAFE_INTEGER
    // remain exact; native number inputs would expose a lossy numeric value.
    const speed = screen.getByRole("textbox", { name: "Speed (Kbps)" });
    expect(speed).toHaveValue("0");
    fireEvent.change(speed, { target: { value: "9223372036854775807" } });
    expect(speed).toHaveValue("9223372036854775807");
    expect(screen.getByRole("spinbutton", { name: "Transmit power (dBm)" })).toHaveValue(-40);
    expect(screen.getByRole("spinbutton", { name: "Channel frequency (MHz)" })).toHaveValue(5180.125);
    expect(screen.getByRole("spinbutton", { name: "Channel frequency (MHz)" })).toHaveAttribute("step", "0.001");
    expect(screen.getByRole("textbox", { name: "Cable positions" })).toHaveValue("1, 1024");
    expect(screen.getByRole("combobox", { name: "Mode" })).toHaveTextContent("q-in-q");
    fireEvent.click(screen.getByRole("button", { name: "Clear Mode" }));
    expect(screen.getByRole("combobox", { name: "Mode" })).not.toHaveTextContent("q-in-q");
    expect(screen.queryByRole("button", { name: "Clear Mode" })).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("spinbutton", { name: "MTU" }), { target: { value: "9000" } });
    expect(screen.getByRole("spinbutton", { name: "MTU" })).toHaveValue(9000);
  });
});

const BAY_TYPE_UUID = "00000000-0000-4000-8000-000000009201";
function BayTypeMembershipForm({ seeded = false }: { seeded?: boolean }) {
  const resource = { id: "mt", name: "SFP", type: "module_type", manufacturerId: "manufacturer", moduleBayTypes: seeded ? [BAY_TYPE_UUID] : [] };
  const initial = buildEntityDraft("Module Types", resource);
  const [draft, setDraft] = useState(initial);
  return <><EntityFormBody entity="Module Types" draft={draft}
    onField={(key, value) => setDraft(current => ({ ...current, [key]: value }))}
    loadOptions={entity => entity === "Module Bay Types" ? [{ id: BAY_TYPE_UUID, name: "SFP bay" }] : []}
    onRequestCreate={() => undefined} />
    <output data-testid="payload">{JSON.stringify(draftToResourceUpdate("Module Types", draft, "Module Type", resource, initial))}</output></>;
}

describe("Module bay membership controls", () => {
  it("clears the last persisted UUID with an explicit empty list", () => {
    render(<AllPermissions><BayTypeMembershipForm seeded /></AllPermissions>);
    fireEvent.click(screen.getByRole("button", { name: "Remove SFP bay" }));
    expect(JSON.parse(screen.getByTestId("payload").textContent)).toHaveProperty("moduleBayTypes", []);
  });

  it("limits the module bay picker to the chosen device, including an inherited hidden device", () => {
    const onField = vi.fn();
    render(<AllPermissions><EntityFormBody entity="Modules" draft={{ deviceId: "device-a", moduleBayId: "" }}
      visibleKeys={new Set(["moduleBayId"])} onField={onField}
      loadOptions={() => [{ id: "bay-a", name: "Slot A", deviceId: "device-a" }, { id: "bay-b", name: "Slot B", deviceId: "device-b" }]}
      onRequestCreate={() => undefined} /></AllPermissions>);
    fireEvent.click(screen.getByRole("combobox", { name: "Module bay" }));
    expect(screen.queryByRole("option", { name: /Slot B/ })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: /Slot A/ }));
    expect(onField).toHaveBeenCalledWith("moduleBayId", "bay-a");
  });
  it("selects UUIDs rather than labels, removes the last selection and preserves untouched membership", () => {
    render(<AllPermissions><BayTypeMembershipForm /></AllPermissions>);
    expect(JSON.parse(screen.getByTestId("payload").textContent)).not.toHaveProperty("moduleBayTypes");
    fireEvent.click(screen.getByRole("combobox", { name: "Module bay types" }));
    fireEvent.click(screen.getByRole("option", { name: "SFP bay" }));
    expect(JSON.parse(screen.getByTestId("payload").textContent)).toHaveProperty("moduleBayTypes", [BAY_TYPE_UUID]);
    fireEvent.click(screen.getByRole("button", { name: "Remove SFP bay" }));
    expect(JSON.parse(screen.getByTestId("payload").textContent)).not.toHaveProperty("moduleBayTypes");
  });
});

const TAG_UUID = "00000000-0000-4000-8000-000000009401";
function TagMembershipForm({ unavailable = false }: { unavailable?: boolean }) {
  const resource = { id: "group", name: "Campus", type: "wireless_lan_group", tags: [TAG_UUID] };
  const initial = buildEntityDraft("Wireless LAN Groups", resource);
  const [draft, setDraft] = useState(initial);
  return <><EntityFormBody entity="Wireless LAN Groups" draft={draft}
    visibleKeys={new Set(["tags"])}
    onField={(key, value) => setDraft(current => ({ ...current, [key]: value }))}
    loadOptions={() => unavailable ? [] : [{ id: TAG_UUID, name: "Core", color: "155dfc", objectTypes: [] }]}
    onRequestCreate={() => undefined} />
    <output data-testid="payload">{JSON.stringify(draftToResourceUpdate("Wireless LAN Groups", draft, "Wireless LAN Group", resource, initial))}</output></>;
}

describe("canonical Tag membership controls", () => {
  it.each([false, true])("preserves omitted membership and explicitly clears the final ID (unavailable=%s)", unavailable => {
    render(<AllPermissions><TagMembershipForm unavailable={unavailable} /></AllPermissions>);
    expect(JSON.parse(screen.getByTestId("payload").textContent)).not.toHaveProperty("tags");
    fireEvent.click(screen.getByRole("button", { name: `Remove ${unavailable ? TAG_UUID : "Core"}` }));
    expect(JSON.parse(screen.getByTestId("payload").textContent)).toHaveProperty("tags", []);
  });

  it("submits the selected catalog UUID rather than its name or slug", () => {
    const onField = vi.fn();
    render(<AllPermissions><EntityFormBody entity="Wireless LAN Groups" draft={{ tags: "" }} visibleKeys={new Set(["tags"])}
      onField={onField} loadOptions={() => [{ id: TAG_UUID, name: "Core", color: "155dfc", objectTypes: [] }]}
      onRequestCreate={() => undefined} /></AllPermissions>);
    fireEvent.click(screen.getByRole("combobox", { name: "Tags" }));
    fireEvent.click(screen.getByRole("option", { name: "Core" }));
    expect(onField).toHaveBeenCalledWith("tags", TAG_UUID);
  });
});

describe("shared Tag catalog form", () => {
  it("renders the actual catalog attributes using shared color and bounded integer controls", () => {
    const draft = buildEntityDraft("Tags", { id: TAG_UUID, name: "Core", type: "tag", slug: "core", color: "155dfc", weight: "0", description: "Network", objectTypes: [] });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    client.setQueryData(["inventory-content-types"], []);
    render(<QueryClientProvider client={client}><AllPermissions><EntityFormBody entity="Tags" draft={draft} onField={vi.fn()} loadOptions={() => []} onRequestCreate={() => undefined} /></AllPermissions></QueryClientProvider>);
    expect(screen.getByRole("textbox", { name: "Name" })).toHaveValue("Core");
    expect(screen.getByRole("textbox", { name: "Slug" })).toHaveValue("core");
    expect(screen.getByRole("textbox", { name: "Color" })).toHaveValue("155dfc");
    expect(screen.getByRole("button", { name: "Open color picker" })).toBeVisible();
    expect(screen.getByRole("spinbutton", { name: "Display weight" })).toHaveValue(0);
    expect(screen.getByRole("spinbutton", { name: "Display weight" })).toHaveAttribute("min", "0");
    expect(screen.queryByRole("combobox", { name: "Tags" })).not.toBeInTheDocument();
  });

  it("keeps identity on rename, omits unchanged attributes and explicitly clears description", () => {
    const resource = { id: TAG_UUID, name: "Core", type: "tag", slug: "core", color: "155dfc", weight: "0", description: "Network", objectTypes: [] };
    const initial = buildEntityDraft("Tags", resource);
    expect(draftToResourceUpdate("Tags", { ...initial, name: "Renamed", description: "" }, "Tag", resource, initial)).toEqual({ id: TAG_UUID, name: "Renamed", description: "" });
    expect(validateEntityDraft("Tags", { ...initial, weight: "-1" }, key => key)).toHaveProperty("weight");
    expect(validateEntityDraft("Tags", { ...initial, color: "url(https://invalid.test)" }, key => key)).toHaveProperty("color");
    expect(buildEntityDraft("Tags").weight).toBe("0");
  });
});
