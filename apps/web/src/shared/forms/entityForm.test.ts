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
    const service: Resource = { id: "s", name: "HTTPS", type: "service", serviceIpAddressIds: ["ip-1", "ip-2"] };
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
