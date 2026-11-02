# SQLx baseline

0001_init.sql is the immutable baseline for the 1.0 schema epoch. It contains
the final schema and factory seeds produced by applying the previous 24-migration
chain, including its data transformations. It is not a concatenation of forward
deltas. Future schema changes append migrations; published bytes within this
epoch remain immutable.

## Canonical Tag references (0006)

Inventory `tags text[]` stores catalog UUID strings, not names or slugs. Tag
identity is immutable; renaming a catalog entry leaves memberships intact.
Native writes reject null, malformed, unknown and duplicate references. Omission
preserves a membership set; `[]` clears it. Referenced definitions cannot be
deleted while inventory owners or persisted target selectors still use them.

Migration 0006 runs with stopped writers in one locked transaction. Each old
token must resolve by exact UUID spelling, slug or name to one catalog identity.
Unknown or ambiguous tokens and duplicate resolved identities abort the whole
migration. It never creates identities for unresolved tokens or detaches their
references. The known native `factory-default` definition is explicitly seeded
at `00000000-0000-7000-8000-000000001000`, preserving the factory memberships
already present in 0001. Conflicting reserved identity, alias or catalog metadata
requires maintenance remediation and aborts the migration.

Executable selectors use `tag_ids`; requests containing retired `tag_slugs` are
rejected even when the value is empty. The migration applies the old selector's
trim and ASCII-lowercase normalization, then requires each Tag predicate to keep
its truth value over every device and VM. This conservatively refuses even a
difference hidden by another filter, preserving conjunctions, limits and
assignment intersections. Pre-existing `tag_ids` keys are refused because the
old selector implementation ignored them.

Definition mutations take the exclusive `inventory-tag-catalog` advisory lock;
membership-only mutations take the shared lock. Mixed sync transactions acquire
the exclusive lock before target planning and write definitions before owners.
Mutations use READ COMMITTED. Read-only REPEATABLE READ assessment remains
supported and is never reused for application.

## Provider source dates

Source `created` and `last_updated` observations belong to the provider-scoped
`sync_inventory_binding.source_created_at` and `source_updated_at` columns, not
local object dates or `piping_synced_at`. Omission preserves a previous
observation; explicit null clears it. Valid instants are normalized to UTC.
Invalid shapes, missing timezone, leap seconds and nonzero precision finer than
PostgreSQL microseconds block preparation without writing rows. Zero fractional
padding is accepted without changing the instant; missing history is not inferred.

## Module bay categories (0005)

`dcim_modulebaytype` already belongs to the baseline. Migration 0005 adds its
memberships on module bays, module types and module bay templates, and enables
bays and templates by default. Deleting a category is restricted while any
membership references it; deleting an owner removes only its own memberships.

The migration refuses dangling graph references, cross-device containment,
cycles, inconsistent parent links and duplicate root names. It neither repairs
these rows nor rebuilds existing nested-set projections or native timestamps.
All DDL and validation run in the migration transaction.

`dcim_modulebay.module_id` identifies the containing module. Its parent bay is
that module's `module_bay_id`; occupancy comes from `dcim_module.module_bay_id`.
Graph writes serialize on the device, and installations also lock their module
type. One graph owner derives new nested-set projections and affected counters.

An empty category set on either side imposes no compatibility restriction.
Otherwise the bay and module type must share a category. Disabled bays reject
new installations, not edits to an existing occupant's metadata. Membership
changes may make an existing occupant incompatible without removing it.

## FHRP auth-key retirement (0004)

Native FHRP keys use consumer and owner scope `inventory_fhrp_group`, group UUID
as both IDs, and field key `auth_key`. Ordinary inventory responses omit this
write-only field. Set and rotate accept a value, never an existing secret ID.
Only `POST /api/v1/inventory/fhrp-groups/{id}/auth-key/reveal` returns
`{"value":"..."}` after concrete-object READ and SECRET checks (including token
scopes and stored group attributes). Reveal is audited without key values and
its response is marked `Cache-Control: no-store`.

Migration `0004_fhrp_auth_key_secret.sql` refuses every nonempty legacy key,
including whitespace-only values, before dropping the plaintext column. Fresh
databases with no legacy values need no conversion. There is no startup fallback,
dual read, automatic conversion or migration-ledger restamping.

The separately packaged converter is `convert-fhrp-auth-keys`. Its explicit
`--offline-confirmed --backup-confirmed --convert-fhrp-auth-keys` arguments are
attestations, not discovery of stopped writers or verified backups. It resolves
the explicit `PIPING_DATABASE__URL` and `secrets.cipher.key_material_ref` through
the same settings owner as the backend. The default cipher reference is
`env:PIPING_SECRETS__CIPHER_KEY_B64`; a configured reference must remain the same
for the backend and converter. Database URLs and key material belong in a private
environment, never in arguments or logs.

The converter locks the group and secret graph, validates the complete cohort,
and uses the canonical encrypted lifecycle in one transaction. Before clearing
plaintext it reads back the exact original bytes and matching owner/usage edges.
It refuses conflicting bindings and whitespace-only invalid keys without changing
them. Its converted count is not proof of conservation of the preexisting graph.
Repeating it against prepared state C is a no-op; after 0004 it refuses the missing
legacy column and never recreates it.

The product-native rehearsal uses exclusively owned synthetic genuine SQLx
prefixes 1, 2 and 3, separate original-R and prepared-C dumps, independent restores,
and the paired backend/converter artifacts. These are not historical installation
fixtures or customer-upgrade acceptance. Public `Deployment.backup` remains
prepared-C-only: it derives the expectation before its dump and cannot capture
nonempty original R. The explicit preparation action below is separate from
backup, local-update and startup; none of them silently converts plaintext.

### Explicit guarded preparation and recovery

This operation supports the local Compose PostgreSQL 18 provider, database and
administrative role `piping`, container port 5432 and `/var/run/postgresql` socket,
one retained data bind/volume, and its native `pg_hba.conf` beneath PGDATA.
`POSTGRES_PASSWORD` must be explicit; file-backed password providers and client
`PG*` overrides other than the stock image's PGDATA/version fields are unsupported.
It requires a trusted maintenance host/Docker daemon,
the pinned Compose writers, unchanged source migration bytes, and enough disk
space for R/C dumps, independent restores, image archives and retained quarantine.
Writer running states are pinned at staging; changing them before quiescence is
a refusal. Recovery configuration bytes, the writer set and the startup label
overlay are authenticated dossier members, not independent local overrides.
Original role images must use locally restorable tags or native image IDs;
repository `name@digest` references are refused during preparation preflight
because native save/load can discard their repository-digest association.
Candidate package images may still be authenticated by their published digest.
Unknown clients, prepared transactions, replication, subscriptions, additional
extensions, preload libraries and external tablespaces are refused. Custom
database providers or layouts require separate implementation and acceptance.

Backend settings must be packaged `config/default.toml` and optional
`config/local.toml`, byte-identical in installed and candidate images. Config
mounts, unsupported settings files and modified installed executables are refused.
Cipher material must come from an explicit `env:NAME` reference represented in
the actual backend environment, including PID1's observed cipher/target values
before it is stopped; entrypoint/environment drift is a refusal. Installed proxy
and backend images must match when proxy is present. The operation never substitutes a default key or
implements encryption. The candidate image must contain both canonical binaries.

Stage the ordinary authenticated package first. Then, on the maintenance host:

```sh
python3 scripts/deploy_release_bundle.py prepare-fhrp-auth-keys \
  --root "$ROOT" --stage "$STAGE" --project-name "$PROJECT" \
  --maintenance-subnet "$OWNED_PRIVATE_IPV4_29"
```

The explicit subnet must be a nonoverlapping private IPv4 `/29`. The operation
stops the pinned writers, refuses unknown sessions and retains a native HBA fence
that rejects external reconnects while allowing only its owned internal client
network. It captures `original-r.dump`, the raw fingerprint, old secret graph,
packaged settings/executable identities and recovery images. An empty owned
target must restore R exactly before preparation pauses at
`original-r-awaiting-proof`. **No target conversion has run at this point.**

Copy the whole private stage dossier to separate disk-backed scratch and use the
existing `verify-backup` action on the independent verifier, with the same package
and verification key. That verifier restores R, runs the paired converter, checks
all old graph rows/ciphertext and new group/owner/usage correspondences, then proves
the remaining SQLx tail. Only after success does it sign `restore-receipt.json`.
The private signing key stays with the verifier, not the installation host.

Copy that receipt/signature back and resume the same stage:

```sh
python3 scripts/deploy_release_bundle.py prepare-fhrp-auth-keys \
  --root "$ROOT" --stage "$STAGE" --project-name "$PROJECT" \
  --maintenance-subnet "$OWNED_PRIVATE_IPV4_29" \
  --receipt "$R_RECEIPT" --receipt-signature "$R_SIGNATURE"
```

The target authenticates R before invoking the converter, checks exact old graph
and unrelated state, compares replay semantics rather than fresh UUIDs/nonces,
and proves repeated C conversion is unchanged. It then uses the shared prepared-C
backup path. Copy this completed dossier to **fresh** verifier scratch and run
`verify-backup` again: R and C must both restore, R conversion must reproduce the
same semantic identities, and sealed C must survive the actual remaining DDL with
exact ciphertext/graph and ledger append. The ordinary `apply` consumes only this
prepared-C receipt; an R authorization cannot authorize apply.

The fence remains through migration and conservation. Original HBA bytes are
restored only immediately before controlled startup. Failed startup or health
checks re-establish the fence and stop recognized writers. A missing or unknown writer cannot skip
stopping other recognized writers or terminating the target database's existing
client sessions; uncertain containment retains deny-all host rules.
If already admitted TCP/authentication sessions cannot be proven drained, the
authenticated pinned PostgreSQL container is stopped rather than left writable.
Explicit R recovery may restart that same contained provider only after checking
its container/storage identity and on-disk deny-all HBA; writers are not restarted.
Existing prepared-C
`backup` uses the same fence owner; it accepts an optional `--maintenance-subnet`
or asks Docker to allocate its owned network. Network allocation failure is a
refusal, not a reason to run unfenced. For isolated restore proofs, `stage`,
preparation, backup and `verify-backup` also accept `--proof-subnet` with an owned
nonoverlapping private `/24`; the owner allocates its bounded `/29` proof networks
from that pool. Without it Docker allocates the private proof networks normally.
There is no retry with a different provider or weaker network policy.

After an authenticated R authorization, explicit recovery is available even if
conversion committed before the preparation process was interrupted:

```sh
python3 scripts/deploy_release_bundle.py restore-original-r \
  --root "$ROOT" --stage "$STAGE" --project-name "$PROJECT"
```

Recovery authenticates R and the original images/configuration, restores into an
empty operation-owned database, verifies complete R, and switches databases under
the fence. The failed database is retained as `piping_quarantine_<operation-id>`
with connections disabled; it is not overwritten or dropped. Writers remain
stopped. `start-original-r` with the same root/stage/project is a separate explicit
action and checks the restored fingerprint before starting the original images.

Do not replay interrupted phases, delete retained artifacts, or force startup
around the journal. Unknown identities, corrupt archives, incomplete recovery or
missing proof leave the operation refused for private diagnosis. A later stage
supersedes the old stage's mutation authority. Installation execution still needs
separate authorization and independent data-risk acceptance; synthetic source and
native receipts do not establish customer readiness.

If an unapplied plain-C update was stopped after backup and an independently
accepted, signed operator recovery has restored the exact old containers and
original access rules, acknowledge that recovery explicitly:

```sh
python3 scripts/deploy_release_bundle.py acknowledge-old-availability \
  --root "$ROOT" --stage "$STAGE" --project-name "$PROJECT" \
  --operator-result "$OPERATOR_RESULT" \
  --operator-result-signature "$OPERATOR_RESULT_SIGNATURE" \
  --verification-key "$VERIFICATION_KEY"
```

This read-only runtime observation authenticates the retained package, both seals
and signed old-availability result. It checks original configuration, schema,
history, provider, HBA, exact old containers and readiness, without comparing live
rows to the now historical backup. A newer helper may authenticate this historical
evidence; it cannot execute backup or apply for a different staged helper.
The action preserves all failed proof evidence and appends the terminal
`aborted-old-availability-restored` state. It neither restarts services nor accepts
the upgrade. A bare terminal marker or incomplete acknowledgment cannot release
the lifecycle block. Any subsequent update requires a new stage, fresh fenced
backup, native restore/conservation proof and independent acceptance.

Migration-seeded content-type metadata has its own exact conservation invariant.
The source-only native prefix upgrade determines the allowed new `(app_label,
model)` pairs under the closed migration manifest. The backup retains every old
registry ID and value. Verification requires those old rows unchanged and exactly
the missing source-declared pairs with new UUIDv7 IDs, then compares the old-row
fingerprint alongside all other protected data. Unknown additions, missing seed
pairs, changed old values or reassigned IDs refuse the update; the registry table
is not exempt from conservation.

### Integrating later schema and data migrations

The original-R rehearsals use producer revision
`198ae7bc0b8c27b84f7ef47a59c17931cb00b485` and its migration tail through 0004.
They do not cover the independently developed 0005 module-bay migration,
0006 Tag canonicalization or a possible 0007 hardware migration. Appending those
files changes the sealed migration manifest, native catalog delta and remaining
SQLx tail; an earlier R/C receipt cannot authorize the changed candidate.

Keep the integration in the existing owners:

- `scripts/ci/postgres_migration_contract.py` owns the exact source-hash-bound
  transform registry, allowed catalog delta and projected columns.
- `Deployment.migration_expectation`, `fingerprint` and
  `load_migration_expectation` own the pre-DDL native row projection and its
  binding to the complete raw backup. Extend them for a reviewed data transform;
  do not exempt changed tables, tags or selector fields from conservation.
- `reference_proof`, `prove_original_r_request`, `restore_proof` and
  `verify_conservation` must replay the complete new tail from the supported
  installed prefixes and verify its schema, rows, ciphertext and ledger append.

The checksum-bound `Tag0006` transform derives its expectation by running only
the canonical Tag SQL on an isolated restore of the pre-DDL state. It projects
each inventory tags array to catalog UUID strings and executable
`selector.tag_slugs` to `tag_ids` before sealing C's expectation. Unknown,
ambiguous or duplicate mappings, previously ignored `tag_ids` and changed target
predicate truth refuse atomically in that isolated transaction. Other columns,
graph/ciphertext and ledger remain conserved, and the resulting complete Tag
row fingerprints compose with the Service/FHRP expectations. The installation is
not modified to calculate the expectation; no second Tag converter is used.

The canonical migration also completes the known factory Tag definition at
`00000000-0000-7000-8000-000000001000` for the `factory-default` memberships
already seeded by 0001. The private projection conserves the complete original
Tag catalog multiset and permits only that exact eight-field row if absent;
an existing exact row remains unchanged. Reserved identity, alias, attribute or
imported-metadata conflicts refuse. The complete resulting catalog hash remains
sealed, so later extra rows, old-row changes/deletions and unknown UUID
substitutions are not authorized by this seed exception.
Pre-seed provenance restricts a newly created factory identity to the explicit
native `factory-default` marker. Its UUID or display name cannot legitimize
previously unknown owner-array or selector values, even with no current targets.
A genuinely pre-existing exact factory row retains its known references.
Owner arrays can use its exact display name. Persisted selectors normalize
`Factory default` to `factory default`, which this canonical migration does not
recognize as an alias, even for an existing row. Use its known UUID or canonical
marker where supported; the helper does not broaden resolver vocabulary.

This extension has synthetic native PostgreSQL proof, not full-tail producer
acceptance. A refusal on a different later migration is an unsupported candidate,
not permission to skip conservation or reuse an old seal.

The integration input supplied for Tag0006 is source
`6101fb88752eb08b3a8e3087f31393c1341331f4`, migration path
`apps/api/migrations/0006_canonical_tag_ids.sql`, SHA384
`433906473689b082c2ea627cd5dd0b0a66b90d9c8c25378b2b89f5af4a20522d9506bc60e1e8edb5227faf7ed37c5859`.
Only that immutable migration is the reviewed input here. Later edits require a
new source binding and proof; a matching filename is not authorization. Full-tail
acceptance additionally needs the paired backend/converter producer carrying the
integrated migration manifest. The 198ae producer cannot supply that proof.

The baseline includes the natural-sort collation, pgcrypto extension, constraints,
indexes, functions, immutability trigger, inventory content types, RBAC policies,
monitoring catalog, system playbook and schedule, branding and license identity.
Generating seed cells retain SQL expressions rather than reference-install values.
Every fresh installation mints its own UUIDv7 license fingerprint. The existing
SQLx query cache describes the same final schema.

## Existing installations: explicit offline transition

Runtime SQLx remains strict. It never repairs checksums, ignores missing versions
or re-stamps an old ledger. Starting this backend against the old chain fails.
Guarded Compose deployment accepts only the already-transitioned baseline cohort;
maintenance is a separate operator action, not an installer fallback.

Its sealed source contract binds both complete native layouts: fresh compact
columns and the 22 original dropped-column slots retained by the recognized full24
transition. This is a fixed source-derived position map across three tables, not
a general position ignore or approval of historical CHECK differences. Mixed or
unknown layouts and every other changed catalog definition remain refused.

The sole supported old cohort is the exact complete successful history 1-24 in
maintenance-transition.json. Versions, descriptions and SHA-384 checksums must
match. Gaps, failed rows, foreign tails, renamed descriptions and older prefixes
are refused before mutation. No pinned-old-image upgrade for shorter histories is
implemented. Do not drop data or rewrite checksums to make them look complete.


The installed EU layout has one additional explicitly sealed representation in
`maintenance-transition.json`: 61 named physical column positions, the exact
CHECK cast representation, and the original pre-semantic SQLx archive. Its two
archived checksums are bound to immutable source `ef45419da210ae83a34ececc8de2bf5738a68e79`.
The transition validates every archive object and both archived history entries,
then projects only those exact from/to positions and the exact CHECK definition.
The complete projected catalog must equal the existing historical digest. This
is not a general position/CHECK ignore or acceptance of an observed digest.
Unknown objects, mixed positions, archive changes and every other catalog change
remain refused. The archive relation and all its rows are preserved unchanged.

Qualify this representation with a verified native physical backup and isolated
restore; a logical dump can compact physical positions and is not evidence of
this exact layout. Keep the completed native backup immutable, verify its
manifest and WAL with `pg_verifybackup`, and retain the original ledger, archive,
image and configuration for rollback. An isolated clone must have no production
volume when started, no TCP listener, and realized network deny-all before
startup. Prove all-table row multisets, archive rows, sequences and native schema
conservation, adversarial refusal, atomic INSERT-failure rollback and repeat
no-op before applying the existing offline transition to the installation.

Maintenance checks PostgreSQL 18, current user-collation versions and the entire
native schema against its source-derived digest. It binds physical column positions,
constraints, indexes, functions, user/internal triggers, extensions, collations and
the other shared deployment catalog objects. Only server-generated ICU version
metadata is omitted from the cross-build digest; stale recorded user-collation
versions are refused. Unknown schema changes require investigation, not a relaxed guard.

1. Stop every writer, including schedulers and old backends. Retain the exact old
   image identity and the installation's configuration/secrets.
2. Make a private pre-transition backup and verify a restore into an exclusively
   owned disposable PostgreSQL instance. Keep its original ledger and all data.
   Do not continue without a usable restore and enough capacity.
3. From this exact checkout, run the command with both acknowledgements. Flags
   precede the command:

   ~~~bash
   bash scripts/rebaseline-sqlx-0001.sh \
     --maintenance-confirmed --backup-confirmed apply-compose -- \
     --env-file /private/install/.env -f /private/install/docker-compose.yml
   ~~~

   For the local development Compose project:

   ~~~bash
   make rebaseline-sqlx-0001 \
     REBASELINE_ARGS='--maintenance-confirmed --backup-confirmed'
   ~~~

   apply-env uses standard libpq variables and PGPASSFILE without a URL argument.
   print-sql emits the same guarded transaction for private inspection or execution
   with psql -X -v ON_ERROR_STOP=1.
4. Start only the consolidated-baseline backend and use the normal guarded
   deployment/health checks. Maintenance success is not application acceptance.

The transaction locks application tables before observing history and schema,
then changes only _sqlx_migrations. It never runs factory SQL or updates user data.
Credentials, ciphertext, entitlement JWTs, license identity, customized seed rows
and sequence state are retained. A SQL error aborts the transaction; no later
success stamp is attempted. Repeating maintenance against a fresh baseline or a
transitioned installation is a no-op only while its recognized schema and ledger match.

### Rollback boundary

An old backend rejects the consolidated ledger despite the logically equivalent
application schema. Do not restart it against the transitioned database. Keep
writers stopped after failure; use an authorized forward repair or restore the
verified pre-transition database together with its old image and configuration.
Never reapply the fresh baseline to an existing database. This source contract
does not authorize a live database transition.

## Native proof

The proof owner is scripts/check_netbox_owner_schema.py. Use an exclusively owned
PostgreSQL 18 container with --network none, no published ports or host socket
mounts. The caller owns readiness and cleanup:

~~~bash
python3 scripts/check_netbox_owner_schema.py \
  --container <isolated-container> --prefix baseline_proof \
  --output-dir /data/artifacts/<unique-proof-directory>
~~~

This checks fresh native schema, Owner/inventory behavior, nonempty baseline
maintenance, conflicting-history/schema refusal and repeated-maintenance conservation.
Add --historical-migrations <immutable-source-export> to apply the exact recognized
24 source files in the disposable database and prove the nonempty old-history
transition, including INSERT-failure rollback after ledger DELETE. This export is
a test input, never a runtime fallback. Historical checksum/CHECK fixtures stay intact.

The proof compares row multisets and sequence state through counts and hashes.
Artifacts and private logs stay outside git. source.json records actual input
hashes and dirty-tree truth. A dirty run validates no commit; --require-clean-source
requires exact HEAD inputs. This direct SQL proof does not replace the backend's
SQLx entrypoint test in tests.compose.test_deploy_release_bundle.NativeMigrationContractTests.

The current CI entrypoint is `scripts/ci-compose-native.py --baseline`. It exports
helpers and application inputs from one committed SHA, verifies the supplied
backend artifact hash/source binding, and passes `PIPING_NATIVE_BASELINE_SHA256`
to the real backend and guarded-deployment tests. Its public receipt binds
`ledger_versions: 1` and `env_only_baseline`. MR proof uses the canonical Dockerfile
builder from a Git export with a generated fixture-only signing key; default-branch
proof consumes the exact backend image-build artifact. Neither path publishes a
release or operates an installed database.

Without `--baseline`, the registered historical CHECK route retains its pinned
helper/application pair. The old23/24 artifact route is frozen at the last
pre-baseline helper, rather than applying a baseline-only helper to old artifacts.
Those old receipts are not baseline acceptance and do not authorize old-history
deployment under the current contract.

The installer regression has a separate explicit scope: current launcher/TLS
inputs with the pinned old24 application images and their matching schema helper.
Its source manifest records both revisions. It does not apply the baseline helper
to old installed images or claim a full installer run for the not-yet-published
1.0.0 image cohort. Current backend/migration and guarded-update acceptance belongs
to the baseline route; final published-image installation remains a release-phase
check owned by the publisher.

## Preserved domain constraints and remaining work

Owner/OwnerGroup are AdminModels, not tag/custom-field models. Their assignment rows
cascade on identity/Owner deletion; inventory ownership is protected. Cooling
templates and FHRP assignments remain ancillary models. Imported source dates are
nullable and are not fabricated from synchronization timestamps. Tag membership
still uses tags text[]; tag conversion and complete API/UI/import support for the
schema prerequisites remain separate work.

Services retain nullable protocol/ports and the ordered mixed-protocol
ipam_service_port_mapping relation. Local targets use typed mapping configurations
and sync_inventory_binding; retired raw-JSON mapping-spec storage is absent.
Maintenance does not convert unsupported older schemas.

Field-secret retirement flags default to false; they do not retroactively grant
cleanup authority over historical variables/bindings. Licensing retains suspended
and the monotonic revision used to reject stale refresh completions.


## Runtime migration to the vmauth query endpoint

A deployed source that still points somewhere other than the canonical estate
query endpoint is migrated through the supported REST write path (direct
database mutation is not a supported write path). The backend reaches the
monitoring stack only through the cluster Service, so the canonical endpoint is
the deployment-agnostic in-cluster URL:

```
http://vmauth.<monitoring-namespace>.svc.<cluster-domain>:8427/api/v1/query
```

On this estate that resolves to the in-cluster URL set as `VMAUTH_QUERY_URL` in
step 0 below. The public `victoria.piping.space` hostname is not routable from
the pod network, so a source pointed at it never answers.

The credential is a **named, non-protected** secret whose value is the vmauth
`query-bearer-token`. The migration never copies a credential value into its
own storage: the source stores only the named secret's UUID, and the value stays
in the secret store.

Operator sequence (the token never touches the shell history or stdout):

```bash
# 0. The canonical endpoint: the form above, resolved for this estate.
VMAUTH_QUERY_URL="http://vmauth.monitoring.svc.piping-eu:8427/api/v1/query"

# 1. Create the named secret; capture only its id.
SECRET_ID=$(curl -fsS -X POST "$API/api/v1/automation-secrets" \
  -H "cookie: $COOKIE" -H 'content-type: application/json' \
  --data '{"key":"vmauth-query-bearer-token"}' | jq -r .id)

# 2. Set the value straight from the token file; the value is never echoed.
curl -fsS -X PUT "$API/api/v1/secrets/$SECRET_ID/value" \
  -H "cookie: $COOKIE" -H 'content-type: application/json' \
  --data "$(jq -n --rawfile value "$TOKEN_FILE" '{value: $value}')"

# 3. Repoint the source at the canonical endpoint and bind the secret UUID.
curl -fsS -X PUT "$API/api/v1/monitoring/query-sources/$SOURCE_ID" \
  -H "cookie: $COOKIE" -H 'content-type: application/json' \
  --data "$(jq -n --arg ref "$SECRET_ID" --arg url "$VMAUTH_QUERY_URL" \
    '{name:"vmauth", slug:"vmauth", source_type:"victoriametrics",
      query_url:$url,
      credential_ref:$ref, timeout_ms:3000, options:{}, enabled:true}')"

# 4. Verify the persisted reference (never the value).
curl -fsS "$API/api/v1/monitoring/query-sources" -H "cookie: $COOKIE" \
  | jq '.items[] | select(.slug=="vmauth") | {query_url, credential_ref}'
```

The write fails unless the secret exists, is `kind = 'named'`, and is not
protected; the bound secret is then deletion-protected (`409 Conflict`).

## Named-secret references

Monitoring sources use one secret_usage_binding row at field_key = 'bearer_token';
playbook projects use field_key = 'scm_credential'. The removed raw credential
columns are not part of the consolidated schema. playbook_project.scm_ref is a
git revision; playbook_job_template.credential_refs contains connection/shared-
credential-set references, not bare named-secret UUIDs.

secret_usage_binding.secret_id cascades on secrets_variable deletion. The application
guard SecretRepository::delete_named refuses deletion with 409 Conflict while bindings
exist; future deletion paths must use that guard.
