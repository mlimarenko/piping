# Dataflow intent and host policy

Inventory owns directed intent, not cables, sessions or deployed firewall rules.
The API neither executes firewalls nor generates playbooks. General Inventory and
Services are not tenant-scoped here; CRUD uses the existing Inventory gate.

## Minimal intent and selectors

`GET/POST /api/v1/inventory/dataflows` and
`GET/PUT/DELETE /api/v1/inventory/dataflows/{id}` expose one model. Minimal create:

```json
{
  "source": {"object_type": "internet"},
  "destination": {"object_type": "dcim.device", "object_id": "00000000-0000-7000-8000-000000000101"}
}
```

Name is optional and derived from endpoint labels when omitted. Description is
optional, status defaults to `draft` (`active`/`disabled` are also valid).
Transport defaults to `any`; `tcp`, `udp` and `sctp` are explicit alternatives.
Ports omitted or `null` mean any ports, including non-port-bearing protocols when
transport is any. A selected port list must contain 1..256 entries in 1..65535;
it is sorted/deduplicated. `[]` is invalid, never an implicit wildcard. Any
transport plus selected ports expands TCP/UDP/SCTP rules, not ICMP-as-port.

Endpoints use flow-owned `DataflowEndpointInput` / `DataflowEndpointResponse`:
`object_type`, nullable `object_id`, `address_mode` (default `default`),
`ipaddresses` (default `[]`); response adds `label`. The global inventory ObjectRef
schema is unchanged. Types are `dcim.device`, `virtualization.virtualmachine`,
`ipam.service`, `ipam.ipaddress`, `ipam.prefix`, `all`, `internet`, plus the native
inventory scopes below. Only builtins
omit/null object_id; inventory references require real UUIDs. Builtins and explicit
IP/Prefix endpoints have fixed default mode. Device/VM/Service modes:

| Mode | Meaning |
| --- | --- |
| default | Device/VM primary management IPs. Service configured IP bindings, otherwise eligible assigned parent-host IPs (native service-any semantics). |
| selected | One or multiple explicit eligible member IP UUIDs, validated against owning object/parent. Other modes cannot carry selected UUIDs. |
| all | Device/VM assigned, primary and declared service-binding IPs. Service configured bindings, otherwise eligible assigned parent IPs. |

Selected service IPs must belong to declared bindings when present; without
bindings the parent host's eligible owned addresses are selectable. This
can include a primary-only parent IP; default/all fallback remains assigned-only.
Picker choices use the same membership rule as explicit selection, not the
default/all fallback address set. Selection
never copies addresses into a second service/deployment model. Existing Service
CRUD exposes `ipaddresses: UUID[]`: create defaults empty, update omission retains
bindings, `[]` clears them. Parent changes revalidate retained bindings atomically.
Services own listener inventory; their native ports/port mappings do not choose
flow transport or allowed ports.
Configured bindings are explicit intent in both default and all modes: an
ineligible, dangling or reassigned binding blocks the entire relevant policy.
Only discovered fallback candidates may be excluded without blocking.

`POST /api/v1/inventory/dataflows/endpoint-resolution` accepts a
`DataflowEndpointInput` body and requires **Read Inventory**, not Create.
It replaces the unreleased address-picker GET directly; there is no alias.
`DataflowEndpointResolutionResponse` returns `endpoint`, `members`, `ipaddresses`,
`default_ipaddresses`, `addresses`, and `diagnostics` from the same repeatable-read
catalog/selector logic used by policy. Picker choices are eligible owned
`{id,address,vrf_id}` records. Actual chosen intent is normalized `{cidr,vrf_id}`.
Each member is `{host:ObjectRef,addresses,diagnostics}`; complete known membership
includes unaddressed/ineligible/not-selected hosts with explanatory diagnostics.
Diagnostics have stable `code` and `message`; consumers translate codes, not raw
messages. Preview makes no `apply_ready`, ConnectionTarget-access or tenant-safety
claim. Builtins and fixed IP/Prefix can be previewed but have no mode picker.

## Native inventory scopes

The same selector supports these UUID-backed inventory scopes; there is no
arbitrary group model or weak polymorphic reference. Migration 0021 adds strong
RESTRICT references without changing published migration 0020.

| object_type | Device membership | VM membership |
| --- | --- | --- |
| dcim.region | own site → region | own site → region |
| dcim.site | own site | own site |
| dcim.sitegroup | own site → group | own site → group |
| dcim.devicerole | own role | own role |
| tenancy.tenant | own tenant | own tenant |
| tenancy.tenantgroup | own tenant → group | own tenant → group |
| dcim.platform | own platform | own platform |
| dcim.manufacturer | own device type → manufacturer | none |
| dcim.devicetype | own device type | none |
| dcim.rack | own rack | none |
| dcim.location | own location | none |
| virtualization.cluster | own cluster | own cluster |
| virtualization.clustergroup | own cluster → group | own cluster → group |
| virtualization.clustertype | own cluster → type | own cluster → type |

Region, SiteGroup, DeviceRole, TenantGroup and Location include self and descendants
through actual `parent_id`, with bounded cycle detection on the selected branch.
MPTT and Cluster scope caches are not membership sources. Platform is flat:
its native table has `parent_id`, but its owning domain does not expose hierarchy.
No inheritance of site/tenant/platform from Site, Cluster or hypervisor is implied.
Default scope selection chooses eligible primary management IPs per member;
all chooses eligible owned IPs; selected chooses 1..32 eligible member IP UUIDs.
Discovered ineligible/unaddressed members are exclusions; explicit unresolved
selection and relevant ambiguity fail closed. Scope/All address budgets are 4096,
not the singleton 256-address budget; each demanded scope catalog is capped at
4096 rows. Unrelated branches do not consume a selected scope's budget.
Overlapping collections retain both real ingress and egress pairs. Local scope
visibility projects to the chosen requested-host addresses; remote visibility
checks only compatible contributing owners. Prefix retains its separate network
contributor semantics. Preview members/addresses are deterministic, never silently
truncated.

PUT changes supplied fields only; explicit `destination_ports:null` clears a port
filter, omission preserves it. Description `""` clears text. List accepts status,
search, source_type/source_id, destination_type/destination_id, limit/offset.
Builtin filters need type only, real-object filters need type and UUID. Sorting
is name then UUID. No bulk endpoint. Generic Inventory MCP CRUD exposes dataflows
without bulk operations. Typed RESTRICT FKs and the selected-IP junction prevent
dangling deletion; deferred constraints keep mode/junction presence consistent.

## All, Internet, identity and routing contexts

`all` is the snapshot's known eligible Device/VM inventory addresses, including
assigned, primary and declared service-binding ownership. It never means 0/0 and
never includes arbitrary unassigned allocations. Unaddressed/ineligible members
contribute none and produce exclusions, not a global failure. Wholly empty All
blocks an applicable flow. Each host's local side is restricted to its own chosen
eligible addresses: All→Host B gives B ingress, Host A→All gives A egress.
Same-host pairs are omitted. Internet has no concrete-host ownership, so it never
makes unrelated hosts relevant simply because they lie in 0/0.

`internet` explicitly means `0.0.0.0/0` and `::/0`, including private IPs; there
is no hand-written public-range classifier. Its wildcard takes the opposite
host's address family and routing context. Individual IPs always resolve /32 or
/128, not stored subnet masks; explicit Prefix keeps normalized CIDR. Prefix
membership uses inventory ownership and VRF, not address-string equality.

Default pairing uses the same family and VRF. All excludes other realms and
explains exclusions. Duplicate numeric IPs in different VRFs are valid inventory,
not inherently ambiguous. Duplicate host ownership in the same exported context
blocks application. `allow_cross_vrf` defaults false; true declares explicit
cross-realm intent while retaining the requested host's VRF in rules. It never
proves routing/NAT feasibility; ambiguous overlapping ownership after cross-realm
projection still fails closed. No route or NAT inference.
Projection checks the remote IP or Prefix against the full requested-host realm
inventory, not just the local host's addresses. A collision with a third host
therefore blocks export too.

## Host-policy export and permissions

`GET /api/v1/inventory/dataflows/firewall-policy?host_type=dcim.device&host_id=UUID`
(also VM) requires Read Inventory and canonical Read ConnectionTarget visibility
for the requested host and contributing device/VM peers, including Service parents.
Default-deny and deny override apply. A hidden relevant peer denies the request;
other-realm All exclusions are not hidden contributors. This is not a universal
Inventory-reader bypass and does not claim tenant isolation.
Prefix contributors are known owners inside its family, VRF and CIDR, including
owners of /32 and /128 prefixes. Ineligible explicit IPs retain typed ownership
for relevance and authorization; they cannot turn unresolved intent into an
apparently ready partial policy.
Remote Prefix contributors with ambiguous same-context ownership block the entire
export. A local Prefix is projected to the requested host's addresses: visibility
and address errors of other local members do not affect that host's export.

The response is `{host,apply_ready,rules,diagnostics}`. Rules contain direction
(ingress/egress), family (ipv4/ipv6), transport (including any), source/destination
CIDRs, nullable destination_ports, nullable vrf_id and sorted dataflow_ids. Only
active relevant flows contribute. Identical rule tuples deduplicate and merge
provenance; output ordering is deterministic. An unresolved relevant explicit
endpoint makes apply_ready false **and rules empty**, not a partial export.
No relevant intent gives ready empty output with no_relevant_flows: this does not
prove a safe default firewall policy. Stateful established/related traffic and
baseline policy remain consumer-owned; no automatic reverse flows/source-port limits.

An Ansible consumer can fetch JSON and refuse unresolved intent before its own
implementation handles protocol/port omission:

```yaml
- name: Fetch intent with operator-supplied authentication
  ansible.builtin.uri:
    url: "{{ piping_url }}/api/v1/inventory/dataflows/firewall-policy?host_type={{ inventory_host_type }}&host_id={{ inventory_host_id }}"
    method: GET
    headers: "{{ piping_auth_headers }}"
    return_content: true
  register: intent
  no_log: true
- name: Refuse unresolved intent
  ansible.builtin.assert:
    that: intent.json.apply_ready | bool
- name: Retain input for the consumer implementation
  ansible.builtin.set_fact:
    host_network_intent: "{{ intent.json.rules }}"
```

For transport any, omit a protocol filter; for destination_ports null, omit a port
filter. This example does not configure a firewall. The read-only repeatable-read
transaction covers flows, selections, objects, addresses, ownership and facets.
Limits: 1000 active flows, 256 singleton object addresses/host memberships, 4096
scope/All addresses and 4096 rules; demanded catalogs cap hosts/services/prefixes
at 4096 and IP rows at 8192. Mutation validates reference existence and explicit
selected IP membership, including ownership and ambiguity witnesses; it does not
expand default/all address sets. Endpoint preview and host policy load the
inventory required by their endpoint selections and active policy flows.
Unrelated inventory growth does not consume those budgets or block a small
direct, service or scoped request. Creating or activating an `all` intent can
succeed even when its preview or active host-policy expansion exceeds a catalog
budget.
Exceeding a demanded catalog budget rejects the request; other unresolved
budgets clear all rules. Demand scoping does not relax ownership validation,
peer visibility checks or ambiguity handling.
