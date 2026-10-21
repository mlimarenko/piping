# License Operations Runbook

## Scope

This runbook covers production operations for commercial licensing between:

- `backend` (self-hosted instance, entitlement consumer)
- `com.backend` (commercial control-plane, entitlement issuer)

It focuses on:

- key rotation
- outage handling
- subscription/entitlement incident recovery

## 1. Signing-key compatibility and coordinated rotation

### Single-key trust boundary

App pins one Ed25519 verification key at compile time. It has no runtime trust
key override, dynamic JWKS refresh or multi-key rollover path. Changing that key
requires rebuilding and redeploying a verified backend artifact. Keeping an old
key in Cloud's key set does not let a single-key App validate a different active
signer.

Both Cloud regions must sign with a key compatible with the deployed App cohort
and issue `iss=piping-com-backend`, `aud=piping-instance`. This applies to both
activation and refresh. Regions with different active keys cannot both serve the
same single-key App artifact successfully.

An incompatible cohort was observed on 2026-10-18:

- App 1.0.124 backend image `sha256:535decb1d00b4af49b5d9865d6ef5e1b8849f7c424d0ab439a25f9db3ab94e4a`
  contained one Ed25519 PEM, SPKI SHA-256
  `c90a5d21adb0c1950200931cc827455c481d96bf26b55328e89f9d210d4bc81b`.
- Both Cloud 1.0.103 regions exposed SPKI SHA-256
  `19aec75db99fb5a85db0432081885059159e31a52ace4431b6f597867394ac76`.

A corrected public-key build input does not alter that already published image.
These are observed artifact fingerprints, not permanent expected keys; obtain
fresh observations before each rollout.

### Operator sequence

1. Read `GET /api/v1/license/public-key` from each actual Cloud region. Compare
   Ed25519 SPKI DER SHA-256 fingerprints, algorithm, issuer and audience. Stop if
   the regions disagree. Do not infer identity from a key name or `kid` alone.
2. For a stale App-key mismatch, leave Cloud signers unchanged. Align App's
   public-key build input `PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM_B64` (preferred)
   or `PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM` with the observed common issuer key.
   Keep variable values out of logs; compare public fingerprints on readback.
3. Rebuild the backend release artifact. Inspect the binary extracted from the
   exact published image digest and identify its single baked licensing key.
   Record the image digest, binary SHA-256 and key SPKI SHA-256. For an extracted
   public PEM, enable shell `pipefail` and require this pipeline to succeed:
   `openssl pkey -pubin -in <public-key.pem> -outform DER | openssl dgst -sha256`.
   Require fingerprint agreement with both Cloud regions before deploying. CI variable
   readback, image labels and debug fixture-key runtime tests are not substitutes
   for this final-artifact check.
4. Deploy only that verified digest. If genuinely rotating Cloud's signer rather
   than correcting an App build input, prepare the matching App artifact first
   and coordinate both Cloud regions and all serving App cohorts in a maintenance
   window. Exclude incompatible cohorts from issuance/refresh. There is no
   guaranteed zero-downtime mixed-key transition: existing cached JWTs signed by
   the old key can also fail verification in a newly pinned App until replaced
   by a successful compatible refresh. Retaining old Cloud keys alone does not
   solve either direction of this incompatibility.
5. Recheck the deployed artifact fingerprint and both issuers, then verify actual
   activation/refresh against each region. Require refresh 200, active license
   state and cleared sync error. Do not delete license state or fall back to
   Community to conceal a trust mismatch.

### Rollback

Restore a compatible Cloud-signer/App-artifact cohort together. A key rollback
alone can strand App instances pinned to the other key. Any chosen App artifact
must also support the already applied SQLx migration ledger; an older binary is
not automatically a safe rollback. No runtime trust fallback is available.

## 2. Commercial Control-Plane Outage

### Symptoms

- `POST /api/v1/license/refresh` fails.
- `license_state.status` becomes `degraded`.
- `license_state.last_sync_error` contains request/verification failures.

### Behavior Expectations

1. Existing users with assigned seats continue to work.
2. New seats remain bounded by the verified signed limit, even below 10.
   Outages enter `degraded`, then the configured `grace` window; new seats are
   blocked during grace or after entitlement expiry. An outage is not revocation.
3. Background refresh worker keeps retrying on configured interval.

### Recovery Steps

1. Restore `com.backend` connectivity.
2. Manually trigger refresh once:
   - `POST /api/v1/license/refresh`.
3. Confirm DB state:
   - `license_state.status = active`
   - `license_state.last_sync_at` updated
   - `license_state.last_sync_error IS NULL`
4. Confirm login flow for a non-seated user (if capacity exists).

## 3. Subscription Incident Recovery

### Confirmed issuer refusal

A recognized Cloud refusal suspends new seat allocations immediately, without
starting or extending outage grace. Existing assigned users keep access. The
persisted `suspended` state and canonical reason survive restart and later outages;
neither deletes the entitlement nor falls back to Community. Health details expose
the issuer reason, not `OK` or a connectivity error. A verified successful fresh
refresh or replacement activation restores allocation under its signed limit.

Authoritative responses are `TOKEN_REVOKED` (403), `TOKEN_NOT_FOUND` (404 from the
resolved instances route after both Cloud homes were checked), `ENTITLEMENT_BINDING_INVALID` (409),
`LICENSE_INSTANCE_CONFLICT` (409), and `ORGANIZATION_ARCHIVED` (403). Activation also
recognizes `TOKEN_ALREADY_CONSUMED` (409). Unknown or malformed responses and
unverifiable JWTs are diagnostics, not issuer revocations; 429, 5xx and timeouts
remain transient even when their body names a refusal code.

Classification uses the emitted HTTP status, envelope `code`, domain
`reasonCode` and operation together. Cloud emits `code: NOT_FOUND` with
`reasonCode: TOKEN_NOT_FOUND`, and `code: CONFLICT` for the three 409 domain
reasons above. Binding and instance conflicts are refresh refusals;
`TOKEN_ALREADY_CONSUMED` is activation-only. The 403 refusals use their domain
reason as both fields. A generic router 404 without the domain reason is not
authoritative. Upstream human-readable messages never select this policy.

Roll out the Cloud producer correction to both homes before this consumer:
an initial local miss when the peer is unavailable or forwarding is disabled
must be retryable 503 (`peer_unreachable`), never a definitive 404.

Refresh results are fenced by a persisted revision. Starting a later refresh or
activating a replacement supersedes both successful and failed older results;
a superseded result returns a conflict rather than claiming a successful write.
Replacement preserves the instance fingerprint and existing seat ledger. Failure
before persistence leaves the previous entitlement unchanged.

### Signed seat-ceiling reduction

Cloud retains the previous signed ceiling during its 7-day downgrade grace.
Only after activation or refresh accepts a signed ceiling below the active seat
count does App evict excess users. The policy is LIFO: the last user to log in
is the first removed, not classic LRU. A successful login by an already seated
user updates their entry time. Ties use assignment time, then seat ID.

Administrators are protected from a full lockout in both activation and refresh, for direct and group RBAC assignments. Ordinary users are evicted
first; administrators are selected only when no ordinary seated user can be
removed. While any seat remains, the oldest remaining administrator is kept,
so a reduction cannot revoke every administrative account. If the signed
ceiling is zero no seat can remain.

App revokes each excess seat, all its user's non-revoked sessions and active API
tokens in the same transaction as the entitlement update. The ledger records
`seat_revoked` with reason `seat_evicted_over_capacity` and the signed `max_seats`.
Older seat holders within the ceiling keep access. Repeating the same ceiling
does not remove more users. Evicted users may log in again if capacity later
exists; revoked sessions and API tokens are not restored.

An outage or issuer refusal alone does not trigger this eviction policy.

### Case A: Seats paid, but login still blocked

1. Check `com.backend` subscription for org:
   - `base_seats`, `extra_seats`, `status`, `period_end`.
2. Trigger backend refresh (`POST /api/v1/license/refresh`).
3. Verify local cache:
   - `license_state.max_seats` equals expected paid seats.
4. Retry blocked login.

### Case B: Entitlement stale/expired

1. Inspect `license_state.entitlement_expires_at`.
2. If expired:
   - refresh license.
3. If refresh fails:
   - inspect `last_sync_error`
   - inspect `com.backend` logs and entitlement signer key config.
4. Resolve signer/config mismatch, then refresh again.

### Case C: Seat assignment drift

1. Check active seat assignments:
   - `SELECT COUNT(*) FROM license_seat_assignment WHERE revoked_at IS NULL;`
2. Check active users:
   - ensure assignments for removed/inactive users are revoked.
3. For emergency unblocking:
   - explicitly revoke stale seat assignments for inactive/deleted users.

### Forced reconciliation flow (control-plane -> instance)

When billing state and entitlement drift persists, run forced reconcile end-to-end:

1. In `com.backend` trigger:
   - `POST /api/v1/platform/organizations/{organization_id}/reconcile`
   - include reason with incident id.
2. Immediately trigger self-hosted instance refresh:
   - `POST /api/v1/license/refresh`
3. Verify:
   - `license_state.status = active` (or expected billing-derived state)
   - `license_state.max_seats` matches `com.backend` subscription seats
   - `last_sync_error IS NULL`
4. If mismatch remains, inspect:
   - `com.backend` `billing_events` ordering and dedup
   - `backend` refresh logs / JWT verification state.

## 4. Verification Queries

### Backend (consumer)

```sql
SELECT instance_id, status, max_seats, entitlement_version, entitlement_expires_at, last_sync_at, last_sync_error
FROM license_state
ORDER BY updated_at DESC
LIMIT 1;
```

```sql
SELECT COUNT(*) AS active_seats
FROM license_seat_assignment
WHERE revoked_at IS NULL;
```

### Com.backend (issuer)

Check:

- latest rows in `entitlement_issuance_log`
- latest `billing_events`
- latest `audit_events` for `billing.*` and `entitlement.issued`

## 5. Alerting Recommendations

1. Alert when `license_state.status = degraded` for > 15 minutes.
2. Alert when `entitlement_expires_at < NOW() + INTERVAL '24 hours'`.
3. Alert when `last_sync_error` is non-null for consecutive refresh cycles.
4. Alert when active seat count is within 1 seat of `max_seats`.

## 6. Release provenance verification

Before rolling out `backend` images that enforce licensing:

1. Verify image signature for the exact digest planned for deployment.
2. Verify build provenance attestation references the same digest.
3. Confirm provenance source repository + commit SHA matches approved release.
4. Confirm bundled config defaults keep production host policy (`https://piping.space`) intact.
5. Fail deployment if any signature/provenance check is missing or mismatched.
