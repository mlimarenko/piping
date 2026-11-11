# Configuration contexts

Configuration contexts provide JSON data for devices, virtual machines and their
connection targets. They use NetBox-style selection and merging semantics;
Piping's native API continues to use UUIDs and scope bindings rather than
NetBox's numeric identifiers and API representation.

## Selection

A context contains `name`, `weight`, `data` and `is_active`. Scope bindings contain
`context_id`, `scope_type` and `scope_id`. A binding selects an inventory object
by UUID; a `tag` binding selects an existing tag definition, not its display name
or slug.
Context data must be a JSON object. Weight is an integer from 0 through 32767,
with a default of 1000, matching NetBox's context-priority range.

Only active contexts contribute data. A context without bindings is global and
applies to every device and virtual machine. An inactive context contributes
nothing, including when it has no bindings.

Multiple bindings in the same category are alternatives (OR). Different
categories are combined (AND). For example, a context bound to sites A and B,
tenant T and tag `edge` applies to an `edge`-tagged device belonging to T at
either A or B. The same context does not apply to another tenant at site A.
The tag must be attached to the device or virtual machine being resolved;
tagging a related site does not implicitly tag its devices.
Tenant and site criteria refer to the resolved device's or VM's own attributes,
not a union of the tenants and sites of related objects.

The selection categories are regions, site groups, sites, locations, device
types, roles, platforms, cluster types, cluster groups, clusters, tenant groups,
tenants and tags. Location and device-type criteria apply to devices, not VMs.
Piping also supports direct `device`, `vm` and `connection_target` bindings as
native extensions. These are independent categories, not aliases for tags or
other inventory scopes.
Region, site-group and location criteria include their ancestor hierarchy.
Tenant-group criteria match only the tenant's immediate group, not parent groups.

To select devices with either of two tags, bind both tags to one context. This
differs from the playbook target selector's `tag_ids`, which requires all listed
tags. Context selection does not reuse that selector's semantics.

## Merge order

Matching contexts are applied in ascending `weight`, then ascending `name` order.
The context's ID is only a final deterministic tie-breaker. More specific scope
bindings do not override this order. Each context contributes at most once,
regardless of how many of its bindings match.

Nested JSON objects merge recursively. Arrays, scalars and null replace the
previous value at the same key. Later contexts win conflicting values. A
device's or virtual machine's `local_context_data` is applied last and overrides
all matching contexts.

For example, a context with weight 100 may supply NTP and syslog servers, while
a matching context with weight 200 replaces only the syslog list. NTP settings
remain unless the later context also replaces them.

## Native API

- `/api/v1/config-contexts` and `/api/v1/config-contexts/{id}` manage context data
- `/api/v1/config-context-bindings` creates and lists scope bindings
- `/api/v1/config-context-bindings/{id}` removes a binding
- `/api/v1/inventory/devices/{id}/config-context` returns a device's merged context
- `/api/v1/inventory/virtual_machines/{id}/config-context` returns a VM's merged context

Context and binding mutations require the existing Config Context permissions.
Unsupported scope types, missing scope targets and malformed UUIDs must be
rejected rather than accepted as ineffective bindings.

Creating an active context without bindings makes it global immediately. When
building a restricted context through separate API calls, create it inactive,
add its bindings, then activate it. Removing its last binding makes an active
context global; deactivate it first if that is not intended.
The web editor starts new contexts inactive to allow this sequence; the native
API can still create an explicitly intended active global context.

## Upgrading existing contexts

Older versions applied no contexts without bindings and combined all bindings
with OR. The NetBox-style rules are different. The forward migration must not
silently turn an unused context into a global configuration or reinterpret an
active cross-category rule.

Before upgrading, review active contexts that have no bindings or have bindings
in more than one category. Deactivate those contexts before applying the
migration. Keep their data and bindings, then review them under the new rules
before reactivating them. To preserve an old OR rule across categories, use
separate contexts for the alternatives; to require the categories together, keep
them in one context. Review equal-weight contexts too: conflicts now follow name
order rather than scope specificity.
Also deactivate and review active cluster-bound contexts when the cluster has
physical device members. Those devices did not inherit cluster contexts in older
versions; the new matcher includes them. Clusters without physical members do
not require this additional deactivation.
Review contexts that previously depended on inherited site or tenant values:
the new criteria match the source device's or VM's own attributes.

Older APIs also accepted scope types that the resolver ignored. Existing
bindings outside the previously effective set (`tenant_group`, `tenant`, `site`,
`cluster`, `role`, `device`, `vm`, `connection_target`) block the upgrade even
when their context is inactive. Record and remove these ineffective bindings
before upgrading, then recreate the intended supported criteria afterward.
Otherwise an old inert `tag` or `region` binding could unexpectedly become
effective as soon as the new resolver starts.

The migration stops if ambiguous active contexts remain. Invalid stored context
data, unsupported scope types and missing scope targets also require explicit
remediation rather than being ignored. A failed migration must leave existing
contexts and bindings unchanged. Do not modify the deployed initial migration
or bypass migration checksums.
Previously accepted negative weights also require review and an explicit valid
weight before upgrading; the migration does not normalize priorities.

Use these read-only queries on the old schema to locate the conditions requiring
review. They return identifiers and metadata, not context data:

```sql
SELECT c.id, c.name, count(b.id) AS binding_count,
       array_remove(array_agg(DISTINCT b.scope_type), NULL) AS categories
FROM config_context c
LEFT JOIN config_context_binding b ON b.context_id = c.id
WHERE c.is_active
GROUP BY c.id, c.name
HAVING count(b.id) = 0 OR count(DISTINCT b.scope_type) > 1;

SELECT DISTINCT c.id, c.name, b.scope_id AS cluster_id
FROM config_context c
JOIN config_context_binding b ON b.context_id = c.id AND b.scope_type = 'cluster'
JOIN dcim_device d ON d.cluster_id = b.scope_id
WHERE c.is_active;

SELECT id, context_id, scope_type, scope_id
FROM config_context_binding
WHERE scope_type NOT IN
    ('tenant_group', 'tenant', 'site', 'cluster', 'role', 'device', 'vm', 'connection_target');

SELECT id, name
FROM config_context
WHERE jsonb_typeof(data) <> 'object' OR weight NOT BETWEEN 0 AND 32767;
```

For a reported missing scope target, check the binding's UUID against its
inventory catalog and explicitly restore the intended target or remove the
ineffective binding. Do not erase all bindings as a shortcut: enabling a context
without bindings now applies it globally.
