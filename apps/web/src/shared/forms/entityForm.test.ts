import { describe, expect, it } from "vitest";
import {
  buildEntityDraft,
  draftToResource,
  draftToResourceUpdate,
  entityHasName,
  NAMELESS_ENTITIES,
  validateEntityDraft,
} from "./entityForm";
import type { Resource } from "@/shared/api/types";

const t = (key: string) => key;

describe.each(["Services", "Service Templates"] as const)("%s port mappings", (entity) => {
  it("round-trips mixed transports as an array through the shared draft", () => {
    const resource: Resource = { id: "s", name: "Mixed", type: "service", portMappings: ["tcp/80", "udp/53", "sctp/123"] };
    const draft = buildEntityDraft(entity, resource);
    expect(draft.portMappings).toBe("tcp/80, udp/53, sctp/123");
    expect(validateEntityDraft(entity, draft, t).portMappings).toBeUndefined();
    expect(draftToResource(entity, draft, "Service", resource).portMappings).toEqual(resource.portMappings);
    draft.portMappings = "TCP/01, UDP/65535";
    expect(draftToResource(entity, draft, "Service", resource).portMappings).toEqual(["tcp/1", "udp/65535"]);
  });

  it.each(["", "tcp/0", "tcp/65536", "tcp/-1", "tcp/1.5", "icmp/80", "80", "tcp/80,", "tcp/80,tcp/80", "TCP/080, tcp/80"])("rejects invalid mappings %j before saving", (value) => {
    const draft = { ...buildEntityDraft(entity), name: "Mixed", portMappings: value };
    expect(validateEntityDraft(entity, draft, t).portMappings).toBe("inventory.resources.fields.portMappingsError");
    expect(() => draftToResource(entity, draft, "Service")).toThrow("Invalid service port mappings");
  });
});

describe("Interface networking forms", () => {
  it("preserves lossless speed text and enforces the native integer limit", () => {
    const draft = { ...buildEntityDraft("Interfaces"), name: "eth0", deviceId: "d", speed: "9223372036854775807" };
    expect(validateEntityDraft("Interfaces", draft, t)).toEqual({});
    expect(draftToResource("Interfaces", draft, "Interface").speed).toBe("9223372036854775807");
    for (const speed of ["9223372036854775808", "1.5", "-1"]) {
      expect(validateEntityDraft("Interfaces", { ...draft, speed }, t).speed).toBe("inventory.resources.form.invalidNetworkingValue");
    }
  });

  it("validates numeric bounds, decimal precision, enum and array entries", () => {
    const draft = { ...buildEntityDraft("Interfaces"), name: "eth0", deviceId: "d", mtu: "65537", rfChannelWidth: "1.0001", wwn: "not-a-wwn", cablePositions: "0, 2", poeType: "invalid" };
    expect(validateEntityDraft("Interfaces", draft, t)).toEqual({
      mtu: "inventory.resources.form.invalidNetworkingValue",
      rfChannelWidth: "inventory.resources.form.invalidNetworkingValue",
      wwn: "inventory.resources.form.invalidNetworkingValue",
      cablePositions: "inventory.resources.form.invalidNetworkingValue",
      poeType: "inventory.resources.form.invalidNetworkingValue",
    });
    expect(validateEntityDraft("Interfaces", { ...draft, mtu: "1", rfChannelWidth: "0.001", wwn: "AA:BB:CC:DD:EE:FF:00:11", cablePositions: "1,1024", poeType: "passive-48v-4pair" }, t)).toEqual({});
  });

  it("keeps unchanged attributes absent and clears every nullable networking field", () => {
    const editing: Resource = { id: "i", name: "eth0", type: "interface", deviceId: "d", mtu: "1500", mode: "access", speed: "0", txPower: "-40" };
    const initial = buildEntityDraft("Interfaces", editing);
    const unchanged = draftToResourceUpdate("Interfaces", initial, "Interface", editing, initial);
    expect(unchanged).not.toHaveProperty("mtu");
    expect(unchanged).not.toHaveProperty("speed");
    expect(draftToResourceUpdate("Interfaces", { ...initial, mtu: "", mode: "", speed: "", txPower: "" }, "Interface", editing, initial)).toMatchObject({ mtu: null, mode: null, speed: null, txPower: null });
  });
});

describe("VM type forms", () => {
  it("seeds fractional defaults and zero memory and sends only changed fields", () => {
    const editing: Resource = { id: "type-1", type: "virtual_machine_type", name: "Small", slug: "small", defaultVcpus: "0.25", defaultMemory: "0" };
    const initial = buildEntityDraft("Virtual Machine Types", editing);
    expect(initial.defaultMemory).toBe("0");
    expect(initial.defaultVcpus).toBe("0.25");
    const changed = { ...initial, defaultVcpus: "" };
    expect(draftToResourceUpdate("Virtual Machine Types", changed, "Virtual Machine Type", editing, initial)).toEqual({ id: "type-1", defaultVcpus: "" });
  });

  it("emits an explicit clear when the VM type picker is empty", () => {
    const draft = buildEntityDraft("Virtual Machines");
    draft.name = "VM";
    expect(draftToResource("Virtual Machines", draft, "Virtual Machine")).toHaveProperty("virtualMachineTypeId", "");
  });
});

/**
 * Regression lock for the "phantom Name field" bug class: an inventory entity
 * whose display label the read side derives from a non-name column (IPAM
 * address/prefix, termination role, composite identity) MUST also be listed in
 * {@link NAMELESS_ENTITIES}. Otherwise the edit form renders a required "Name"
 * field whose value the backend silently drops on save — the operator edits it,
 * sees "saved", reopens, and finds the old derived value. These were the six
 * that had drifted out of sync with the read-side derivation in api.ts.
 */
describe("entityForm nameless entities", () => {
  it("seeds and explicitly clears service traffic address bindings", () => {
    const service: Resource = { id: "s", name: "HTTPS", type: "service", portMappings: ["tcp/443"], serviceIpAddressIds: ["ip-1", "ip-2"] };
    const draft = buildEntityDraft("Services", service);
    expect(draft.serviceIpAddressIds).toBe("ip-1,ip-2");
    expect(draftToResource("Services", draft, "Service", service).serviceIpAddressIds).toEqual(["ip-1", "ip-2"]);
    draft.serviceIpAddressIds = "";
    expect(draftToResource("Services", draft, "Service", service).serviceIpAddressIds).toEqual([]);
  });
  const NAMELESS_ON_THE_WIRE = [
    "IP Addresses",
    "Prefixes",
    "Tunnel Terminations",
    "Circuit Terminations",
    "Rack Reservations",
    "MAC Addresses",
  ] as const;

  it.each(NAMELESS_ON_THE_WIRE)("treats %s as nameless", (entity) => {
    expect(NAMELESS_ENTITIES).toContain(entity);
    expect(entityHasName(entity)).toBe(false);
  });

  it("does not require a name for a nameless entity", () => {
    // The address/prefix is required instead; name carries no error.
    const draft = buildEntityDraft("IP Addresses");
    const errs = validateEntityDraft("IP Addresses", draft, t);
    expect(errs.name).toBeUndefined();
  });

  it("synthesizes the label and never emits an edited name for a nameless entity", () => {
    const editing = { id: "ip-1", name: "10.0.0.5/24", type: "ip addresses" } as Resource;
    const draft = buildEntityDraft("IP Addresses", editing);
    // Simulate the operator typing into a (now-removed) name input: it must not
    // round-trip into the saved resource — the prior derived label is preserved.
    draft.name = "operator-typed-this";
    const resource = draftToResource("IP Addresses", draft, "IP Address", editing);
    expect(resource.name).toBe("10.0.0.5/24");
  });

  it("still requires and round-trips the name for a named entity", () => {
    expect(entityHasName("Devices")).toBe(true);
    const draft = buildEntityDraft("Devices");
    expect(validateEntityDraft("Devices", draft, t).name).toBeTruthy();
    draft.name = "edge-router-1";
    const resource = draftToResource("Devices", draft, "Device");
    expect(resource.name).toBe("edge-router-1");
  });

  it("rejects invalid hex colors before a resource is sent to the API", () => {
    const draft = buildEntityDraft("Cables");
    draft.name = "uplink-a";
    draft.color = "#12g";

    const errs = validateEntityDraft("Cables", draft, t);
    expect(errs.color).toBe("inventory.resources.form.colorField.invalid");

    draft.color = "#abc";
    expect(validateEntityDraft("Cables", draft, t).color).toBeUndefined();
  });
});
