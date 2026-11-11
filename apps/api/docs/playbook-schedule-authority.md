# Schedule authority

A schedule runs under its owner, the authenticated user who created it. The
`owner_user_id` field stores that owner; current user status and access-control
policy determine the owner's rights. Service principal identifiers remain on
historical run-group rows, not as schedule execution authority.

Authorization is checked at four points:

1. Saving a schedule checks the active owner against the prepared launch.
2. Enqueueing a scheduled run checks the active owner and current policy again.
3. Launch and dispatch re-check the live owner and the prepared launch.
4. Result writeback checks the authority required for the resulting operation.

The existing launch and secret gates continue to enforce execution and
credential delegation permissions for targets and secrets. Protected secrets
are refused. A missing or inactive owner, revoked permission, or failed secret
gate explicitly refuses the affected operation; it must not report partial
success or fall back to privileged cron or a service principal.

A refused scheduled fire is terminal for the schedule, not retried: when the
owner lost authority, the scheduler disables the schedule, records
`owner_access_revoked` in its metadata and writes a
`playbook.schedule.owner_access_revoked` lifecycle audit event. Transient
storage failures keep the existing retry behaviour.
`0021_playbook_schedule_owner.sql` archives the retired service-principal id in
metadata, pauses schedules whose owner was missing or inactive instead of
deleting or reassigning them, drops the service-principal column and renames the
delegating-user column and its constraints to the owner vocabulary.

## Paused schedules and rollback

The cutover migration pauses a schedule (`enabled = false`) when its owner is
missing or inactive and records `owner_migration_paused = true`,
`owner_migration_pause_reason` and `migrated_from_service_principal_id` in the
schedule metadata. It deliberately does not record the previous `enabled`
value: a paused row means "needs an explicit owner decision", never "this
schedule was disabled before". Do not treat the paused state alone as data loss
or as proof of the earlier setting; read the metadata and the current owner
state.

Supported restoration, one row at a time:

```sql
SELECT id, name, owner_user_id,
       metadata->>'owner_migration_pause_reason' AS pause_reason,
       metadata->>'migrated_from_service_principal_id' AS retired_principal
FROM playbook_schedule
WHERE metadata->>'owner_migration_paused' = 'true'
ORDER BY name;
```

1. Establish the intended owner. The archived retired principal only explains
   who executed the schedule before the cutover; it never becomes an executor
   again. If the previous owner left, a named administrator takes ownership
   deliberately, not by matching names or guessing.
2. Point `owner_user_id` at an active user through the owning repository path
   and then save the schedule through the normal update endpoint. The save runs
   the same launch and secret gates as a manual run, so an owner without
   execute or credential-delegation rights is refused there.
3. Enable the schedule with a valid `next_run_at` only after that save-time
   authorization succeeds. Record the decision (`owner_restored_at`,
   `owner_restored_by`) in metadata and in the audit trail.
4. Never enable a schedule whose owner is inactive, missing or fails the launch
   gate: the scheduler would disable it again on the next fire and the row would
   keep an unresolved authorization state.

Schema rollback does not restore service-principal execution. The archived
metadata and the migration SQL are enough to reconstruct the retired identity
for an audit, but the canonical executor stays the owner: there is no
privileged cron or service-principal fallback to restore.

