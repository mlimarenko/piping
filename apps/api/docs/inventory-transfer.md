# Inventory transfer contract (PWA-81)

The transfer path moves one sealed NetBox capture into the typed Piping
inventory. It has one writer, one planner and one descriptor identity. Nothing
here replaces the assessment commands, and no report authorizes an import.

## Owners

| Concern | Owner |
| --- | --- |
| Descriptor/receipt wire contract, plan and artifact digests | `src/contexts/sync/domain/inventory_transfer.rs` |
| Intake, target CAS, authorization, apply/resume routing | `src/contexts/sync/application/inventory_transfer.rs` |
| Inventory mutation, run fence, durable receipt | `src/contexts/sync/infrastructure/persistence/local_inventory_target.rs`, migration `0012_inventory_transfer_run.sql` |
| Closed target catalog | `src/contexts/sync/domain/inventory_target_schema.rs` |
| Command routing | `src/bin/inventory_transfer.rs` |

## Commands

```
export      --snapshot-transport FILE --provider-id UUID --operator-id UUID
            --endorse-provenance-digest SHA --output NEW_DIR
capture-http --url URL --provider-id UUID --exporter-source-sha SHA --output NEW_DIR
             [--collection-grants FILE --authority-evidence FILE]
descriptor  --bundle DIR --bundle-digest SHA --provider-id UUID --mapping FILE
             --profile FILE --profile-digest SHA --run-id UUID --operator-id UUID
            --approval FILE --scope FILE --endorse-provenance-digest SHA --output NEW_FILE
apply       --descriptor FILE --descriptor-digest SHA --bundle DIR --mapping FILE
            --profile FILE --endorse-provenance-digest SHA
resume      (same flags as apply)
```

`export` consumes a private snapshot transport; it never contacts NetBox.
`capture-http` is the GET-only assessment capture and has no export alias.
`export`, `capture-http`, `preflight` and `dry-run` do not write inventory.
`descriptor` observes inventory in a `READ ONLY` transaction, but authenticating
its operator uses the canonical token service, which refreshes token usage
metadata. It does not mutate inventory. `apply` and `resume` are the only
inventory-writing commands and
they never bootstrap settings, run migrations, seed data, schedule work or
construct a NetBox client. `resume` differs from `apply` only in the reported
code: both share one idempotent path keyed by the descriptor digest.

Environment (never printed):

```
INVENTORY_TRANSFER_NETBOX_TOKEN    capture-http
INVENTORY_TRANSFER_DATABASE_URL    dry-run, descriptor, apply, resume
INVENTORY_TRANSFER_REDIS_URL       apply, resume (provider lease only)
INVENTORY_TRANSFER_OPERATOR_TOKEN  descriptor, apply, resume (bearer token)
```

Exit `0`: completed without blockers. Exit `2`: completed refusal (`blocked`,
`stale`, `busy`, `authorization_lost`, `in_progress`, `uncertain`,
`rolled_back`). Exit `1`: operational or input failure before a decision.

## Descriptor

`TransferDescriptor` is strict JSON (`deny_unknown_fields`). A missing key is a
decode error; an unsupported value — for example a source authority other than
`trusted_synthetic` - is rejected. The only attested v2 authority is
`operator_endorsed_snapshot`; there are no aliases or v1 intake.

| Field | Source |
| --- | --- |
| `run_id`, `operator.user_id`, `approval`, `scope` | operator decision |
| `provider_id`, `bundle_digest`, `mapping_ref`, `mapping_digest`, `profile_digest`, `catalog_digest`, `tool_digest` | verified preflight report |
| `artifact_sha256` | `ImmutableInventorySource::artifact_digest`, computed by the source owner from every verified original payload before resource bytes are discarded |
| `executable_sha256` | digest of the running executable, recomputed and compared at apply |
| `target.identity_sha256`, `schema_digest`, `target.fingerprint_sha256`, `target.plan_digest` | `LocalInventoryCatalog::transfer_observation`, the canonical read-only target observation |
| `authority` | `SourceAttestationIdentity` of the operator-endorsed snapshot, copied from the verified preflight report |
| `retain_absence` | closed enum; only `retain_and_report` |

The run identity is SHA256 of the **exact descriptor bytes**, pinned
independently by the operator. `PinnedTransferDescriptor` carries those bytes,
their digest and the parsed value together. Re-encoding the parsed value is
never used as identity, because JSON round-tripping normalizes whitespace and key
order. A changed byte, a changed digest, an unknown field or a non-current
authority window rejects the run before any connection is used.

`authority` is the canonical `SourceAttestationIdentity` from
`inventory_export`: `authority_id`, `attestation_sha256`, `issuer`, `issued_at`,
`scope_digest`, `expectations_sha256`, `edge_coverage`, `operator_id` and
`provenance_sha256`. The identity is derived, not supplied by the operator.
Preparation copies it from the verified preflight report, which
produces it only when the sealed attestation matches the independently pinned
profile anchors (operator, exact provenance digest, issuer, scope digest and
expectations digest); `apply` then requires
the descriptor's copy to equal the currently verified one. A descriptor cannot
establish authority by naming its own identifier, and a live capture has no
attestation to copy. Export requires an explicit endorsement of the SHA256 of
the exact transport bytes. Descriptor preparation, apply and resume require that
digest again and authenticate the same operator with the canonical token service.

`approval` is a separate `TransferApproval { not_before, expires_at }` window for
the run: the attestation says who owns the snapshot and what it covers, while the
approval says until when this run may still mutate a target. The approval gates
every mutation path and is re-checked inside the writer's fence; it does not gate
returning an already committed receipt.

## Writer boundary

```rust
LocalInventoryCatalog::transfer_receipt(
    run_id: Uuid,
    descriptor_digest: &str,
) -> Result<Option<TransferReceipt>, AppError>;

LocalInventoryCatalog::apply_transfer(
    batch: LocalInventoryBatch,
    descriptor: &PinnedTransferDescriptor,
    authorisation: Arc<dyn InventoryCommitAuthorisation>,
) -> Result<TransferReceipt, AppError>;
```

The writer persists the exact incoming bytes' digest, not a reserialization
digest, and registers the run before mutating inventory. Inventory, bindings,
terminal job state and the receipt commit in one data transaction.

`LocalInventoryCatalog::transfer_observation(&batch) -> Result<(TransferTarget, String), _>`
is the only target identity, fingerprint, resolved-plan and schema observation.
`ImmutableInventorySource::artifact_digest` is the only artifact identity: the
application never serializes records or reconstructs payload bytes.
The trust pins its result verbatim in the descriptor and re-checks the same
values before acquiring the run fence; it never derives a competing identity.

`TransferReceipt` carries `format_version`, `run_id`, `descriptor_digest`,
`outcome` (`committed`), `sync_job_id` (always present on this path),
`committed_at`, `target_fingerprint_sha256`, `stats` and `retained_absent`.

`retained_absent` is the explicit absence report: one entry per source record
absent from the sealed source whose bound target row was retained, as
`source_entity`, `external_id_sha256` (raw identity is hashed at the boundary and
never stored) and `local_id`. Absence is reported, never pruned.

## Authorization

The operator token is validated by the canonical user API token service (hash,
active status, expiry, revocation, active user) at the commit boundary. The
token's user id must equal `descriptor.operator.user_id`. The pinned `scope`
decides the required permissions:

| Scope | Required |
| --- | --- |
| every run, including empty scope | `update × inventory`, the canonical Source UPDATE permission |
| `creates` | `create × inventory`, collection-wide |
| `updates` | `update × inventory`, collection-wide |
| `owner_assignments` | `read × inventory` and both, because a missing owner or owner group may be created |
| `custom_fields` | `update × inventory` |
| `object_references` | `read × inventory` and `update × inventory` |
| `secret_intents` | refused: `inventory_transfer:secret_scope_unsupported` |

Component-template validation also requires object-level Inventory READ against
each resolved target UUID and its persisted attributes. The commit authorizer
refreshes the token, principal and scope permissions at that callback; a global
collection grant or a lease renewal does not override an object-conditional deny.

Before any inventory write, the nine attribute-aware component-template kinds
require their canonical object-level CREATE or UPDATE decision. CREATE uses the
proposed native record with resolved references and canonical defaults. UPDATE
checks both the persisted record and the proposed final attributes. These checks
run inside the same graph-locked transaction for transfer and ordinary pull.
The pull's operation-level `execute × inventory` grant does not replace the
template's mutation grant, and no Source authority overrides a conditional deny.
Changing a positioned port-template mapping also requires UPDATE on its old and
new persisted front-port owners. A new front in the same batch must pass CREATE.

Secret custom-field values are rejected by the supported importer upstream and
there is no secret-intent writer, so a declared `secret_intents` scope is refused
rather than granted or silently written. The effective scope never claims a
secret intent.

The declared scope is a claim, not the requirement. The prepared batch is
inspected through the canonical row owner: `PreparedRow::has_field` reports the
actual mapped intent (scalar, reference, junction, tag or explicit clear) and
`PreparedRow::has_custom_fields` reports a supplied custom-field object. Physical
columns are never used to guess intent. The effective scope is the union of the
declared and observed effects: any mapped foreign key, generic reference,
content-type junction or tag membership requires `object_references`; any
`users.owners` / `users.owner-groups` row or owner reference requires
`owner_assignments`; a supplied custom-field object or a custom-field
definition/choice row requires `custom_fields` **and** `object_references`,
because resolving definitions and Object/Multiobject targets reads inventory
metadata even when no ordinary foreign key, junction, owner or tag is mapped. False booleans can add
requirements but never remove one. A
prepared descriptor stores that union, and `apply` recomputes it from the same
batch and requires an exact match: effects that would need more than the pinned
scope are a different run, reported as `scope_mismatch`, never a silent
widening.

Resolving referenced, owner, tag or custom-field target/definition rows is an
inventory read and requires the collection-wide `read × inventory` grant above.
The component-template callbacks add object-level decisions; they do not replace
that batch-level requirement. Both checks fail closed.

Authorization is not a one-time gate. `apply` passes the writer a composite
`InventoryCommitAuthorisation`: every `ensure_owned` call, including the writer's
final boundary, first checks the provider lease and then re-validates the current
token and re-evaluates the same permissions. A token that expires or is revoked
mid-run cannot commit.

Business owners and owner groups are inventory data, not access-control users.
They are authorized by the same collection grant as any other inventory row and
are never resolved as principals.

### NetBox v4.7.1 owner boundary

Source of truth:
`https://raw.githubusercontent.com/netbox-community/netbox/v4.7.1/netbox/users/api/serializers_/owners.py`
and `netbox/users/models/owners.py`.

| Source | Supported |
| --- | --- |
| `users.owners` | `name`, `description`, `group` and inventory owner references |
| `users.owner-groups` | `name`, `description` |

Populated `users` / `user_groups` memberships stay **blocked**: migrating them
would require inventing an auth-identity conversion. Source owner tables carry no
tags, custom fields or change dates. Populated object custom-field defaults stay
**blocked** until a qualified-default conversion exists; actual object and
multiobject custom-field values are supported.

## Replay and target continuity

Order in `apply`: descriptor bytes and authority window, token and permissions,
durable receipt of this exact run, then live evidence. Consequences:

- a completed same-run request returns the stored receipt before any target
  comparison and before the authority-window check — the window gates writes,
  not evidence retrieval — and never rewrites later edits;
- an incomplete capture, an altered artifact, a different executable, a changed
  mapping/profile/catalog/tool, a different destination database or applied
  schema, a changed fingerprint or a changed plan is refused — the transfer is
  never silently re-planned;
- a `busy` or `in_progress` run writes nothing; `uncertain` is resolved through
  the same run id.

`ProviderLease::run` may drop the work future while `transaction.commit()` is in
flight, for example when lease renewal fails. Such a failure is never stamped as
a rollback and never overwrites a completed job: the application re-reads the
durable run fence and returns `committed_recovered` when a receipt exists, and
otherwise reports `uncertain` or `authorization_lost`. Only a validation refusal,
decided before any row is written, is reported as `rolled_back`.

Run states are a closed vocabulary: `committed`, `replayed`, `blocked`, `stale`,
`busy`, `authorization_lost`, `in_progress`, `uncertain`, `rolled_back`.

## Sealed source authority

`apps/api/src/contexts/sync/domain/inventory_export.rs` owns the sealed bundle
format, `format_version` 2. The manifest carries `authority`:

| `authority.kind` | Attestation | Assessment result |
| --- | --- | --- |
| `live_capture` | none | `source_authority_unattested` blocker |
| `operator_endorsed_snapshot` | `operator_id`, exact `provenance`, `issuer`, `issued_at`, exact `scope`, `edge_coverage`, `expectations` | attested only when the profile pins the same values |

A seal proves integrity, not trust: whoever writes the bytes writes the claim.
Therefore the assessment (`application/inventory_preflight.rs`) accepts a
snapshot bundle only when the supplied `PreflightProfile.snapshot_authority`
pins the same `operator_id`, `provenance_sha256`, `issuer`, `scope_digest` and
`expectations_digest`. The profile's
own digest is pinned out of band, so the claim cannot authorize itself.
A live capture status — including a complete, consistent one — never does.

`authority.snapshot.scope` must equal the census exactly, so a capture cannot
narrow its claim. The expectations document is strict JSON with one entry per
record (`value_sha256` of the exact record values) and one entry per reference
edge. `ImmutableInventorySource::open` re-derives both from the captured
records and rejects the bundle on any extra, missing or changed record, value or
edge, before exposing a single record. Existing incomplete captures remain
decodable and stay blocked. A `format_version` 1 bundle is refused at open
instead of being reinterpreted, so older evidence cannot be relabelled as
authoritative.

Attested edge coverage is `typed_inventory_references`: every
reference the canonical catalog and the captured custom-field definitions imply
must be declared with the entity that field targets.

- ordinary foreign keys: object, bare scalar id or id list, checked against the
  catalog's target entity — an equal id in another entity never satisfies it;
- generic references: the record's `app.model` type token must resolve to an
  allowed entity, and that entity is the declared target;
- junction and tag lists: every element is an edge to the catalog's target (or
  to `extras.tags` for native tag membership);
- object/multiobject custom fields: the captured definition supplies the exact
  related type, and scalar, object and list values are all edges of that type;
- content-type scope metadata (`related_object_type`, `object_types`) is
  validated as a known `app.model` token and is never turned into a collection
  row.

Refused rather than assumed: a populated custom field with no captured
definition or with unresolved type evidence, a reference-shaped value the
catalog cannot type, and any declared edge the field does not imply. A field the
catalog marks as an out-of-scope reference (`target: none`) is accepted only with
an existing declared target and has no catalog type to compare — the one named
residual in this contract.

`ImmutableInventorySource::artifact_digest()` is the Merkle identity of the
verified artifact: the exact manifest bytes plus every verified payload digest in
path order, computed from the original bytes before record payloads leave the
reviewable metadata map. It is the value a descriptor pins as `artifact_sha256`,
and it is unaffected by the metadata API keeping only non-record evidence.

`SourceAuthority::attestation_identity()` derives the immutable identity a run
descriptor pins: `authority_id` (UUIDv5 over the attestation digest),
`attestation_sha256`, `issuer`, `issued_at`, `scope_digest`,
`expectations_sha256`, `edge_coverage`, `operator_id` and `provenance_sha256`.
It is derived from verified fields,
so a descriptor cannot establish authority by naming its own UUID or digest; the
profile's independent anchors decide trust first. `PreflightReport.authority`
exposes this identity only when those anchors matched (`attested`), and exposes
none otherwise. Live captures stay unattested: neither their capture status nor
their artifacts authorize an import.

`PreflightReport.plan_digest` digests the exact assessed `TargetPlanAssessment`:
fingerprint scope, target fingerprint, sorted identities and dependencies,
findings and cycle verdicts. `pg_current_snapshot()` text is a transient
observation and is excluded, so two reads of an unchanged target agree.

## NetBox 4.7.1 snapshot producer

`scripts/netbox_snapshot_export.py` runs inside the pinned NetBox installation.
It configures a core-only Django registry before `netbox.settings` can load and
never imports the source Python configuration, plugins or script serializers.
Connection values come from the private process environment, not command-line
arguments. `contracts/netbox-4.7.1-source.json` is the shipped model, field and
relation disposition contract; schema drift is refused.

The backend image ships the producer and its contract together under
`/usr/local/share/piping/inventory-transfer/`. Copy that directory to the pinned
NetBox installation, preserving the `scripts/` and `contracts/` layout, and run
the producer with NetBox's Python environment. It does not run in the backend
container, which has no Django installation. The image also ships the Rust
`inventory-transfer` executable at `/usr/local/bin/inventory-transfer`.

The producer holds one PostgreSQL `REPEATABLE READ READ ONLY` transaction for the
census, values, relations and process evidence. Credential-bearing fields and
auth credential models are excluded before value projection and serialization.
The resulting internal transport is private source data, not a sealed bundle.
Transfer its exact SHA256 independently, review the dispositions, then pass it
to `export --snapshot-transport` with the operator's explicit endorsement.

The Rust capture checks the shipped contract, complete resource and process
census, consistency evidence, provider identity and exact transport digest. It
uses the same JSON-to-`SourceRecord` converter as HTTP reads, derives the typed
expectations with the immutable source owner, and uses `seal_bundle` for v2.
The transport itself is retained as a digest-verified provenance payload.
Unknown empty collections remain recorded; populated unsupported collections or
references are refused rather than dropped.

Selected typed custom-field values travel with their selected definitions and
content-type scope. A populated unsupported definition reference, including
`choice_set`, blocks sealing instead of losing the original constraint. Generic
ORM type/id pairs are projected to the catalog's canonical reference names
before edge reconciliation; raw ORM id companions and type-only records are
refused at snapshot intake. Service `port_mappings` arrays retain their values
under a typed shape check.

The version contract's `generic_pairs` covers every exported GenericForeignKey,
including `InventoryItemTemplate.component`. The producer checks these pairs
against Django's actual relation descriptors before reading values. A complete
null pair means no reference; populated identities with missing, half-null or
unsupported content-type evidence are refused. The shared converter keeps source
integer/object identities, and typed reconciliation qualifies the edge by the
snapshot provider and exact target entity, not by a local UUID.

Defaults of selected safe custom-field definitions are validated by the same
declared type as their values. Scalar/list defaults and null are retained;
populated Object/Multiobject defaults remain unsupported and are refused before
emission rather than disappearing from the snapshot.

## Not established by this document

Source implementation is not runtime acceptance. Generic live-source trust,
customer credentials and rotation, and FHRP conversion remain out of scope. A
green source build does not imply that any real target can be imported.
