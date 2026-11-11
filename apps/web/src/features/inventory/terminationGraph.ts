/** Pinned native choices shared by graph forms and their request adapters. */
export const VIRTUAL_INTERFACE_TYPES = ["virtual", "lag", "bridge"] as const;
export const VIRTUAL_TERMINATION_ROLES = ["peer", "hub", "spoke"] as const;
export const L2VPN_TYPES = ["vpls", "vpws", "epl", "evpl", "ep-lan", "evp-lan", "ep-tree", "evp-tree", "vxlan", "vxlan-evpn", "mpls-evpn", "pbb-evpn", "evpn-vpws", "spb"] as const;
export const L2VPN_STATUSES = ["active", "planned", "decommissioning"] as const;
export const L2VPN_ENDPOINT_TYPES = ["dcim.interface", "virtualization.vminterface", "ipam.vlan"] as const;

/** Decimal strings preserve the complete signed bigint identifier range. */
export function parseL2VPNIdentifier(value: unknown): string | null | undefined {
  if (value === null || value === undefined) return value;
  if (typeof value !== "string" || !/^-?(?:0|[1-9][0-9]*)$/.test(value) || BigInt(value).toString() !== value || BigInt(value) < -9223372036854775808n || BigInt(value) > 9223372036854775807n) {
    throw new Error("Invalid L2VPN identifier");
  }
  return value;
}
