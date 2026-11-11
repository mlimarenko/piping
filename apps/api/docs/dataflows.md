# Dataflow intent and host policy

Inventory owns directed intent, not cables, sessions or deployed firewall rules.
The API neither executes firewalls nor generates playbooks. General Inventory and
Services are not tenant-scoped here; CRUD uses the existing Inventory gate.

## Flows and selectors

`GET/POST /api/v1/inventory/dataflows` and
`GET/PUT/DELETE /api/v1/inventory/dataflows/{id}` expose one model. Minimal create:

```json
{
  "sources": [{"object_type": "internet"}],
  "destinations": [{"object_type": "dcim.device", "object_id": "00000000-0000-7000-8000-000000000101"}]
}
```

Each side has 1..32 ordered selectors. Requests use `sources` and `destinations`
arrays, including for a single endpoint; singular source/destination fields
are rejected. Resolution considers each directed source/destination pair without
duplicating the original flow identity or provenance.

Name is optional and derived from endpoint labels when omitted. Description is
optional; status defaults to `draft` (`active`/`disabled` are also valid). A flow
can reference an application and a flow group. Effective status accounts for
ancestor groups; assigning or resuming a group never activates a draft flow.

Transport defaults to `any`; alternatives are `tcp`, `udp`, `tcp_udp`, `sctp`,
`icmp` and `icmpv6`. Source and destination ports are separate nullable arrays.
Omitted/null means no port restriction. A selected port list contains 1..256
entries in 1..65535 and is sorted/deduplicated. `[]` is invalid. Transport `any`
with either port filter expands to TCP/UDP/SCTP; `tcp_udp` expands only to
TCP/UDP. Without a port filter, `any` remains unrestricted and includes
non-port-bearing protocols.

ICMP uses `icmp_types`, not a port field: null means any type, otherwise 1..256
unique values in 0..255. ICMPv4 produces IPv4 rules and ICMPv6 produces IPv6 rules.
Ports are invalid for ICMP, and ICMP types are invalid for other transports.

Endpoints use flow-owned `DataflowEndpointInput` / `DataflowEndpointResponse`:
`object_type`, nullable `object_id`, `address_mode` (default `default`),
`ipaddresses` (default []), and tag_object_type only for tags. Responses add label.
The global inventory ObjectRef schema is unchanged. Types include dcim.device,
virtualization.virtualmachine, ipam.service, ipam.ipaddress, ipam.prefix,
ipam.iprange, extras.tag, inventory.addressgroup, all, internet, and native scopes
below. Only builtins omit/null object_id; inventory references require real UUIDs.
Builtins and explicit IP/prefix/range endpoints have fixed default mode.

| Mode | Meaning |
| --- | --- |
| default | Device/VM primary management IPs. Service configured IP bindings, otherwise eligible assigned parent-host IPs (native service-any semantics). |
| selected | Explicit eligible member IP UUIDs validated against the owning object/parent. Other modes cannot carry selected UUIDs. |
| all | Device/VM assigned, primary and declared service-binding IPs. Service configured bindings, otherwise eligible assigned parent IPs. |
| oob | Explicit device OOB IP, with no fallback to primary; not valid for VMs or services. |

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
`DataflowEndpointResolutionResponse` returns `endpoint`, `members`, `ipaddresses`,
`default_ipaddresses`, `addresses`, and `diagnostics` from the same repeatable-read
catalog/selector logic used by policy. Picker choices are eligible owned
`{id,address,vrf_id}` records. Actual chosen intent is normalized `{cidr,vrf_id}`.
Each member is `{host:ObjectRef,addresses,diagnostics}`; known membership includes
unaddressed/ineligible/not-selected hosts with explanatory diagnostics. Stable
diagnostic codes are for consumers; messages are not an authorization contract.
Preview makes no apply-ready, ConnectionTarget-access or tenant-isolation claim.

An explicit IP range resolves to the exact full configured interval as CIDRs,
including remote addresses that are not assigned to known hosts. It is not
widened to a containing subnet and does not enumerate every address. Known
ownership still controls relevance and peer visibility; the local side is
restricted to the requested host, as for a prefix.

A tag selector uses `object_type:"extras.tag"`, the tag UUID as object_id, and
`tag_object_type:"dcim.device"` or `"virtualization.virtualmachine"`. Device and VM
selection are separate; include both selectors to select both kinds. Default
mode selects primary addresses, all selects eligible addresses, and oob is valid
only for device tags. Selected mode is invalid for tags. Other endpoint types
must omit tag_object_type.

## Applications and groups

The catalog uses these REST families under `/api/v1/inventory`:

| Resource | Collection | Main fields |
| --- | --- | --- |
| Application role | `/application-roles` | name, description, comments, owner_id |
| Application | `/applications` | name, role_id, description, shared metadata |
| Flow group | `/dataflow-groups` | name, parent_id, application_id, status, shared metadata |
| Address group | `/address-groups` | name, members, shared metadata |

Collections support GET/POST; `/{id}` supports GET/PUT/DELETE. Shared metadata
uses the existing owner, tenant, tags, comments and custom_field_data facilities.
Application roles have only the fields listed above; they do not have tenant,
tags or custom fields. Catalog metadata does not establish tenant isolation.

Catalog lists accept `search`, `limit` and `offset`, plus the applicable owner,
tenant and tag filters. Applications also accept `role_id`; flow groups accept
`application_id`, `status`, `parent_id` and `root=true`. Matching totals are
computed before pagination.

Application contacts use `/contact-assignments`. Create an assignment with
`object_type:"inventory.application"`, the application's `object_id`, and both
`contact_id` and `role_id`. For a scoped GET, use `object_id` and `content_type`,
the UUID resolved from `/content-types`, not the content-type token. The UI's
Application contacts link opens this scoped list and prefills the application
when creating an assignment. Referenced applications cannot be deleted.

`custom_field_data` follows the shared custom-field definitions for the entity's
content type. Unknown, out-of-scope, missing required and invalid typed values
are rejected. Secret fields use the shared secret store; list, detail and mutation
responses mask their values. Omitting this field on update preserves its values.

Object references and metadata use `inventory.applicationrole`,
`inventory.application`, `inventory.dataflowgroup`, `inventory.addressgroup`
and `inventory.dataflow`. The address-group token is also used by flow selectors.

Flow groups form a bounded, cycle-checked hierarchy. Children share their
parent's application; a flow in an application-bound group must use that
application. Conflicting changes are rejected instead of silently moving other
records. Flow groups default to active; flows still default to draft.

An ancestor in disabled or draft state prevents an otherwise active flow from
contributing rules. Disabled takes precedence over draft, then active. Responses
include `effective_status` and `blocked_by_group_id`; the latter identifies the
blocking ancestor when the row's own state does not already explain the result.
Pausing/resuming a group never changes its children's stored statuses.
`inherited_tags` contains ancestor tags; `effective_tags` is their sorted union
with direct tags.

Address groups have 1..64 ordered members using the same selector model as
flows. A member cannot reference another address group. Flows reference one
with `object_type:"inventory.addressgroup"` and its UUID. The wrapper uses default
mode; each member owns its address mode. All members resolve in the policy
snapshot, with existing unresolved and budget guards.

Deleting a referenced role, application, flow group or address group returns
409. Dependency details are returned only when their visibility is authorized.
Remove or reassign references first; deletion
does not cascade into flows or change their effective status.

## Native inventory scopes

The same selector supports these UUID-backed inventory scopes with typed
RESTRICT references:

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

## Filters and bulk actions

PUT changes supplied fields only. Omitting a side preserves it; supplying
`sources` or `destinations` replaces the whole side in the transaction. Empty
sides are invalid. Null clears a nullable traffic, group or application field;
omission preserves it. Description `""` clears text.

The flow list accepts `status`, `effective_status`, `search`, `application_id`,
`group_id`, `transport`, `tag_id`, `source_type`/`source_id`,
`destination_type`/`destination_id`, and `limit`/`offset`. Group filtering includes
descendants; tag filtering uses effective tags. Builtin endpoint filters need
type only; real-object filters need type and UUID. The envelope is
`{items,total,limit,offset}`; total covers all matches, with name/UUID sorting.
The default limit is 50 and the maximum is 1000.

For extras.tag endpoints, `source_tag_object_type` or
`destination_tag_object_type` optionally narrows that side to device or VM tags.
It requires the corresponding extras.tag type and tag UUID. Without this scope,
the endpoint filter matches either host kind for the tag. This selector filter
is distinct from tag_id, which filters metadata tags attached to flows/groups.

`POST /api/v1/inventory/dataflows/bulk-action` accepts, for example:

```json
{"ids":["00000000-0000-7000-8000-000000000201"],"action":"set_status","status":"disabled"}
```

Actions are `set_status`, `move` and `delete`. Set-status requires a status. Move
requires `group_id`, `application_id` or both; null explicitly clears a value.
Unrelated action fields are invalid. Delete requires Delete Inventory; the other
actions require Update Inventory. Between 1 and 1000 IDs are accepted. The server
locks the unique IDs in deterministic order and commits all changes together.
An unknown ID, conflicting application/group or invalid row rolls everything
back. Success is `{action,affected:[{id}]}`, returned after commit.

Generic Inventory MCP exposes catalog CRUD; generic bulk-create is unsupported.
Typed RESTRICT references prevent dangling
selectors; deferred constraints keep selected mode and selected-IP rows consistent.

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
(ingress/egress), family (ipv4/ipv6), transport, source/destination CIDRs, nullable
source_ports, destination_ports and icmp_types, nullable vrf_id and sorted
dataflow_ids. Only effective-active relevant flows contribute. Identical tuples
deduplicate and merge provenance; output ordering is deterministic. An unresolved
relevant explicit endpoint makes apply_ready false **and rules empty**, not a
partial export. No relevant intent gives ready empty output with no_relevant_flows:
this does not prove a safe default firewall policy. Stateful established/related
traffic and baseline policy remain consumer-owned; reverse flows are not inferred.

## Read policy from Ansible

External consumers use a personal API token with `read:inventory`. The token is
a permission ceiling, not a grant: its owner must also have Read Inventory and
Read ConnectionTarget permission for the requested host and every contributing
peer. This Inventory endpoint does not require a separate ConnectionTarget token
scope. Create the token in personal settings and keep it in the consumer's
secret store. Do not put its value in inventory files or playbook source.

Piping system playbooks with `inventory_rest_enabled` receive a short-lived run
token. A policy read requires `inventory:read`, the run's exact device or VM,
and the initiating user's or service principal's live Inventory and Connection
Target permissions. A different host, a hidden peer or a revoked grant denies
the request. A token stops working when it expires, is revoked or its run ends.
Copied and user-authored playbooks do not acquire the system capability by
copying playbook content; they can use a separately authorized API token from
the product's secret variables.

The endpoint is the same in both cases. For a Piping system run, take the URL
from `PIPING_API_BASE_URL`, the token from the current host's `piping_api_token`
and the host ID from `piping_source_id`. Map `piping_source_kind=device` to
`dcim.device` and `virtual_machine` to `virtualization.virtualmachine`. Never use
one host's run token for the other hosts in a grouped launch.

This read-only example accepts an operator-supplied URL, host selector and
authentication headers. It refuses unresolved or empty intent before returning
rules to the consumer. A caller that intentionally accepts empty intent must
make that decision separately; an empty result does not define a default policy.

```yaml
- name: Fetch intent with operator-supplied authentication
  ansible.builtin.uri:
    url: "{{ piping_url | regex_replace('/+$', '') }}/api/v1/inventory/dataflows/firewall-policy?host_type={{ inventory_host_type | urlencode }}&host_id={{ inventory_host_id | urlencode }}"
    method: GET
    headers: "{{ piping_auth_headers }}"
    return_content: true
    status_code: 200
    timeout: 30
  register: intent
  delegate_to: localhost
  no_log: true
- name: Refuse unresolved or empty intent
  ansible.builtin.assert:
    that:
      - intent.json.apply_ready is sameas true
      - intent.json.rules | length > 0
    fail_msg: "No apply-ready rules were returned. Inspect the policy diagnostics."
    quiet: true
- name: Retain input for the consumer implementation
  ansible.builtin.set_fact:
    host_network_intent: "{{ intent.json.rules }}"
  no_log: true
```

For transport any, omit a protocol filter. A null source_ports or destination_ports
means omitting that side's port filter; null icmp_types means no ICMP type filter.
Preserve each rule's family, direction and VRF, and retain `dataflow_ids`
when producing a change report. A successful API response is not evidence that
rules are installed. This example does not configure a firewall. The read-only
repeatable-read transaction covers flows, selections, objects, addresses,
ownership and facets.
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
