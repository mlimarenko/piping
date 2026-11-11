# NetBox source sync

The NetBox source is read-only. A provider row names the endpoint and stores its
API token as a secret binding; every pull resolves that credential once, and the
resolved revision is revalidated with the provider state and the pinned mapping
inside the commit transaction. There is no write-back path: webhook/event-rule
provisioning and outbound pushes exist only for `bidirectional` mappings, and a
`pull_only` provider refuses them even when a queued event is replayed.

## What one pull observes

A pull traverses the complete compiled catalogue of the target, not a subset. The
collector refuses a page whose count drifts, a repeated cursor, a duplicate
identity or an incomplete response, and it only issues completed-manifest
evidence when every collection reached a terminal page — an observed empty
collection is different from an omitted one. That evidence is bound to the
provider, the immutable mapping version and the exact record digest.

An optional namespace (the Data Flows plugin) is discovered from the live schema.
When the plugin is not exposed, its collections are withheld rather than observed
empty: the core catalogue still pulls, the mapping needs no optional entities, and
a previously imported plugin keeps its rows while the capability is unavailable.
A plugin that is present must be fully traversed and fully mapped; a schema or
permission failure fails the pull instead of silently dropping its entities.

## Absence and deletion

Absence is a separate, deliberately slower decision:

1. the first complete traversal that does not select a proved unit records a
   pending absence;
2. a second consecutive complete traversal of the same configured source
   generation confirms it;
3. seeing the unit again clears the pending row, and any credential, provider or
   mapping change starts the count over.

Only then, and only under the mapping's `delete_source_owned` policy, may the
unit be removed. Removal additionally requires the unit to be source-created:
the writer mints a proof only when it observed the native row absent and created
it in the same transaction, and stores a digest of the persisted state. A local
edit, a delete-and-recreate, a conflicting owner or a proof whose native
generation moved relinquishes eligibility permanently — a later pull cannot
re-adopt it by value equality. Rows that predate the proof, including every row
of the sealed transfer path, stay unproven and are never deleted.

The native side keeps its own budget: every inbound effect is read from
PostgreSQL's catalog, a retained child under any action (including `CASCADE` and
`SET NULL`) blocks the removal, composite keys are counted and blocked instead of
being given an invented identity, and the complete set is deleted children-first
in one savepoint with deferred checks after the whole set. Derived trees, device
and virtual-machine counters and prefix bookkeeping are recomputed for the
removals as they already are for upserts.

Any conflict rolls back the entire pull — inventory rows, bindings, proofs,
absence counters and job completion — and the sanitized reconciliation report is
written afterwards so the failure stays visible. Under `retain_and_report`, the
job reports pending and confirmed absences and applies no absence-driven removal.

## Operating notes

* A mapping's absence policy is immutable per version; changing it creates a new
  version, which resets the source generation and therefore the confirmation
  count.
* The configured token's visible inventory is the authoritative universe. An
  object that merely became invisible to the token is indistinguishable from an
  object deleted in NetBox; review token scope before choosing a destructive
  policy.
* Schedules fire the same pull runner, persist their mapping version and
  principal, and skip a fire while the provider lease is held. A revoked or
  disabled principal fails at the deferred commit boundary instead of applying a
  partial pull.
