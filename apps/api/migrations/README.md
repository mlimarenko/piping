# SQLx baseline

0001_init.sql is the only runtime migration for the 1.0 schema epoch. It contains
the final schema and factory seeds produced by applying the previous 24-migration
chain, including its data transformations. It is not a concatenation of forward
deltas. Future schema changes append migrations; published bytes within this
epoch remain immutable.

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
python3 -m unittest tests.ci.test_netbox_owner_schema tests.ci.test_sqlx_rebaseline -v
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
