# Playbook catalog contract

The shipped catalog is `ansible/playbooks/piping-playbooks.yaml`
(`apiVersion: piping.space/playbook-catalog/v1`). It is the single source of
builtin playbook content. The startup loader in the playbooks context is the
only writer of builtin project, revision and job-template identity; the public
project, file, job-source and job-definition mutations are read-only for that
source.

## Identity

Identity is deterministic and product-owned. It never derives from an instance
hostname, URL, display name, user-editable value or a random UUID.

| Object | UUIDv5 name | Notes |
| --- | --- | --- |
| Catalog project | `https://piping.space/playbook-catalog/v1/project` | one project for the whole catalog |
| Catalog job template | `https://piping.space/playbook-catalog/v1/job/<slug>` | one template per entry |
| Catalog revision | `https://piping.space/playbook-catalog/v1/revision/<tree_sha256>` | one immutable revision per catalog tree |
| Non-system entry id | `https://piping.space/playbook-catalog/v1/job/entry/<slug>` | used only when the entry omits `id`; the name comes from `catalog_entry_id` |

The namespace is the RFC 4122 URL namespace
`6ba7b811-9dad-11d1-80b4-00c04fd430c8`. System entries (`system: true`) must not
pin `id`; their template identity is always the slug-derived name. A non-system
entry may pin `id` for historical compatibility. Two entries resolving to one id
fail the whole document.

If an object with the canonical id exists with a different name/source, or a
project with the same display name exists under a different id, the loader fails
closed. It never adopts or overwrites a user project by name.

## Source and read-only

`playbook_project.source` is a JSON object (`{"kind":"editable"}`,
`{"kind":"git",...}` or `{"kind":"system","key":"piping.space/playbook-catalog/v1"}`).
The `source` column has no kind check; adding `system` is migration-free. The
public project, file-revision and job-source mutation endpoints reject `system`
projects, and creating a job template inside the catalog project is rejected.
Only the startup loader creates or updates those rows.

The immutable builtin definition and the instance settings are one documented
split, shared by the startup seed and the API guard:

| Catalog-owned (immutable) | Instance-owned (configurable, preserved) |
| --- | --- |
| project, name, workdir | execution environment |
| playbook paths | revision selection (`current` or `fixed`) |
| declared parameter schema | parameter overrides (`extra_vars`) |
| approved launch arguments (`options.argv`) | runtime env, credential bindings (`secret_refs`) |
|  | targets (`inventory`), run timeout (within the product ceiling) — the catalog timeout is only the default of a new row |

An API update that changes a catalog-owned field is rejected, and so is moving a
builtin job to another project: immutability is decided from the persisted row's
own project, never from the proposed one. A catalog re-seed rewrites only the
catalog-owned fields and preserves every instance setting, including the
execution environment an operator retargeted, and it never resets a changed run
timeout: a catalog timeout change only affects rows created afterwards. The
configured `default_execution_environment_id` is only the seed value of a new job
row, so removing it never hides or resets an existing job.

## Launch options

A catalog entry declares launch settings only through owners the product already
has:

- `timeout_seconds` is the entry field and becomes the job template's run
  timeout (1 s to the product run ceiling);
- `launch_options.argv` is the single approved vocabulary and is validated and
  normalized by `piping_contracts::ansible_options`, the same owner the API and
  runner use for launch arguments.

Any other key — `forks`, `strategy`, `stdout_callback`, `shell_executable`,
`ssh_common_args`, `remote_tmp`, `pipelining`, `execution_backend` and the rest
of the previous catalog document — has no consumer in the product. Declaring one
fails the whole document (`deny_unknown_fields`); it is not silently dropped and
there is no second option converter. Ansible-side defaults (forks, callbacks,
connection security) remain owned by the runner configuration, so the catalog
can neither force a Windows-incompatible shell nor override connection security.

## Manifest validation

The loader validates the whole manifest before any mutation and commits the
update atomically:

- `apiVersion` equals `piping.space/playbook-catalog/v1`;
- every `slug` is lower-case ASCII and unique;
- `catalog.base_path`, `entrypoint_file` and `vars_schema_file` are safe relative
  paths with no absolute prefix, backslash, `.` or `..` segment;
- a system entry does not pin `id`; no two entries share an id;
- `language` is `yaml`;
- `input_schema`, `output_schema` and `token_policy` are JSON objects;
  `token_policy.scopes` only names the closed product scope set and has no
  duplicates;
- `launch_options` contains nothing outside the approved launch matrix;
- the catalog tree contains no symlinks; every referenced entrypoint exists;
  every referenced vars-schema file exists and parses as JSON;
- the catalog revision tree hash is computed by the existing revision owner
  (`snapshot_tree_sha256`) over the exact shipped files, not by a bespoke walker.

The loader is bounded before it validates: at most 2000 files, 8 MiB in total,
1 MiB per file and 1 MiB for the manifest; the sizes are checked from file
metadata before a read. A failure leaves the previous catalog state intact:
builtin jobs report `catalog_not_loaded` with the logged reason instead of
half-seeded objects. A human project already using the catalog display name is a
collision, never an automatic adoption.

## Seeding

Seeding runs once at application start, after migrations, from
`playbook_runtime.catalog_path` (default `/app/ansible/playbooks`, the image
path). The transaction takes an advisory lock keyed by the catalog project id, so
two instances starting at once serialize instead of racing a read-then-insert;
the second start observes the first one's committed rows and adopts them. One
transaction writes the project, at most one new immutable revision and the
system job templates. A repeated start with an unchanged tree writes
nothing; a changed tree appends a new revision, moves
`playbook_project.current_revision_id`, and leaves the previous revision and any
launched run group untouched.

The project and revision are always written. Job templates are written only for
an explicitly configured execution environment
(`playbook_runtime.default_execution_environment_id`) that exists and is
enabled: the canonical job model requires one, and the loader never fabricates
an environment, a digest or an owner fallback. An entry that leaves the manifest
is not deleted; its slug stops resolving and existing runs keep their rows.

## Revision and lookup

One immutable `playbook_project_revision` is created per catalog tree hash. New
jobs select `current`; operators can choose `fixed` with a revision of the same
system project. Reseeding preserves the selection: current follows the catalog,
fixed keeps the selected snapshot. The selected snapshot must contain the job's
catalog-owned entrypoint and compatible parameter schema. No fallback is used.
Each dispatch captures its revision and the existing exact-revision vars-schema
lookup (E1) is reused.

`GET /api/v1/playbooks/catalog/{slug}` resolves the builtin job by catalog slug
behind the existing job-template read permission. It reports the template id and
truthful availability:

- `catalog_not_loaded` — the startup seed failed; the reason is in the startup log;
- `execution_environment_not_configured` — no default environment is configured
  and no job row exists yet;
- `template_not_seeded` — an environment is configured but the job row is absent;
- `execution_environment_unavailable` — the row's environment is missing or
  disabled (the row's environment, not the configured default);
- `targets_not_configured` — no targets are attached to the job. A seeded template starts with
no instance targets, so the lookup says `targets_not_configured` until an
operator configures them; it never claims a launchable job it cannot launch.
- `revision_unavailable` — the selected snapshot is missing or incompatible
  with the job's current catalog-owned entrypoint or schema.

## Migration status

The `system` source kind is carried in the existing `source` jsonb and adds no
column or constraint, so PIP-553 needs no migration. The live chain is
`0001..0024`; `0025` remains free.
