#!/usr/bin/env python3
"""Prove the consolidated App baseline against isolated PostgreSQL 18.

The caller starts the isolated container/service. Native schema and shipped
Owner/inventory behavioral assertions remain required. Historical fixtures are
immutable evidence, not a second runtime chain or permission to rewrite history.
SQLx ledger insertion is in the successful migration transaction only.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
MIGRATIONS = ROOT / "apps/api/migrations"
FIXTURES = ROOT / "apps/api/tests/fixtures/netbox"

IDENTIFIER = re.compile(r"[a-z_][a-z0-9_]*\Z")
HISTORY_DDL = """
CREATE TABLE _sqlx_migrations (
    version bigint PRIMARY KEY,
    description text NOT NULL,
    installed_on timestamptz NOT NULL DEFAULT now(),
    success boolean NOT NULL,
    checksum bytea NOT NULL,
    execution_time bigint NOT NULL
);
"""


def identifier(value: str) -> str:
    """Reject unsafe identifiers at the Docker/SQL command boundary."""
    if not IDENTIFIER.fullmatch(value):
        raise ValueError("Expected a lowercase SQL identifier")
    return '"' + value + '"'


def manifest() -> dict[int, pathlib.Path]:
    paths = sorted(MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql"))
    result = {int(path.name[:4]): path for path in paths}
    if len(result) != len(paths):
        raise ValueError("Duplicate migration version")
    if not result or sorted(result) != list(range(1, len(result) + 1)):
        raise ValueError("Migrations must be a nonempty contiguous history starting at 1")
    return result


def checksum(path: pathlib.Path) -> str:
    return hashlib.sha384(path.read_bytes()).hexdigest()


def validate_baseline() -> None:
    """The single runtime baseline replaces, rather than aliases, the old chain."""
    paths = manifest()
    if list(paths) != [1] or paths[1].name != "0001_init.sql":
        raise ValueError("Expected exactly the consolidated 0001_init.sql runtime migration")



def source_provenance(require_clean: bool = False) -> dict:
    """Observe Git and actual proof inputs; a dirty tree validates no commit."""
    def git(*args: str) -> bytes:
        return subprocess.check_output(
            ["git", "-c", f"safe.directory={ROOT}", "-C", str(ROOT), *args],
            stderr=subprocess.PIPE, timeout=10)

    head = git("rev-parse", "HEAD").decode().strip()
    if not re.fullmatch(r"[0-9a-f]{40}", head):
        raise ValueError("Invalid observed Git commit")
    changes = git("status", "--porcelain=v1", "--untracked-files=all").decode().splitlines()
    paths = set(manifest().values()) | {
        FIXTURES / "owner_admin_model.sql", FIXTURES / "owner_admin_relations.sql",
        ROOT / "scripts/check_netbox_owner_schema.py",
        ROOT / "scripts/ci/postgres_migration_contract.py",
        ROOT / "tests/ci/test_netbox_owner_schema.py", MIGRATIONS / "README.md",
        MIGRATIONS / "maintenance-transition.json",
        ROOT / "scripts/rebaseline_sqlx.py", ROOT / "tests/ci/test_sqlx_rebaseline.py",
        FIXTURES / "netbox_forward_schema.sql",
    }
    actual = {}
    committed = {}
    for path in sorted(paths):
        relative = path.relative_to(ROOT).as_posix()
        actual[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
        try:
            committed[relative] = hashlib.sha256(git("show", f"{head}:{relative}")).hexdigest()
        except subprocess.CalledProcessError:
            # New dirty inputs have no committed provenance, never a valid SHA.
            committed[relative] = None
    matches = actual == committed
    clean = not changes and matches
    result = {"observer": "git_worktree", "observed_head": head,
              "worktree_dirty": bool(changes), "worktree_changes": changes,
              "files_match_head": matches, "validated_source_sha": head if clean else None,
              "file_sha256": actual, "head_file_sha256": committed}
    if require_clean and not clean:
        raise ValueError("Final proof requires a clean tree whose input bytes match HEAD")
    return result


def proof_output_directory(output: pathlib.Path, postgres_host: str | None) -> pathlib.Path:
    """Resolve the output boundary before creating files or contacting PostgreSQL."""
    if output.is_symlink():
        raise ValueError("Proof output must not be a symlink")
    resolved = output.resolve()
    if postgres_host is not None:
        if (resolved.parent != ROOT / "apps/api/coverage"
                or not re.fullmatch(r"\.netbox-schema-[1-9][0-9]*", resolved.name)):
            raise ValueError("Native CI proof output must be job-scoped coverage scratch")
    elif not resolved.is_relative_to("/data") or resolved.is_relative_to(ROOT):
        raise ValueError("Proof output must be outside the tree under /data")
    return resolved


class Proof:
    def __init__(self, container: str | None, output: pathlib.Path,
                 require_clean: bool = False, postgres_host: str | None = None):
        if (container is None) == (postgres_host is None):
            raise ValueError("Choose exactly one isolated PostgreSQL transport")
        if container and not re.fullmatch(r"[a-z][a-z0-9-]*", container):
            raise ValueError("Invalid container name")
        if postgres_host and postgres_host != "postgres":
            raise ValueError("Native transport is restricted to the isolated CI postgres service")
        output = proof_output_directory(output, postgres_host)
        output.mkdir(parents=True, exist_ok=False, mode=0o700)
        self.container = container
        self.postgres_host = postgres_host
        self.output = output
        self.sequence = 0
        self.source = source_provenance(require_clean)
        self.write("source.json", self.source)
        if container:
            info = json.loads(self.command(["docker", "inspect", container]))[0]
            if info["HostConfig"]["NetworkMode"] != "none" or info["HostConfig"]["PortBindings"]:
                raise ValueError("Proof requires network none and no published ports")
        if self.scalar("postgres", "SHOW server_version_num")[:2] != "18":
            raise ValueError("Proof requires PostgreSQL 18")

    def command(self, argv: list[str], data: bytes | None = None,
                expected_failure: str | None = None) -> bytes:
        result = subprocess.run(argv, input=data, capture_output=True, timeout=120, check=False)
        self.sequence += 1
        log = self.output / f"command-{self.sequence:04d}.log"
        log.touch(mode=0o600)
        log.write_bytes(result.stderr)
        if expected_failure:
            if not result.returncode or expected_failure.encode() not in result.stderr:
                raise RuntimeError(f"Expected transactional rejection missing; private log: {log}")
        elif result.returncode:
            if b"AdminModel forbidden metadata columns" in result.stderr:
                raise RuntimeError("AdminModel forbidden metadata columns (corrected assertion RED)")
            raise RuntimeError(f"SQL proof command failed; private log: {log}")
        return result.stdout

    def sql(self, db: str, text: str, expected_failure: str | None = None) -> bytes:
        identifier(db)
        return self.command(self.postgres_command("psql", db) + [
            "-X", "-qAt", "-v", "ON_ERROR_STOP=1",
        ], text.encode(), expected_failure)

    def postgres_command(self, program: str, db: str) -> list[str]:
        argv = [program, "-U", "postgres", "-d", db]
        if self.container:
            return ["docker", "exec", "-i", self.container, *argv]
        return [*argv, "-h", self.postgres_host]

    def scalar(self, db: str, sql: str) -> str:
        return self.sql(db, sql + ";").decode().strip()

    def json(self, db: str, sql: str):
        return json.loads(self.scalar(db, sql))

    def create(self, db: str):
        self.sql("postgres", f"CREATE DATABASE {identifier(db)};")
        self.sql(db, HISTORY_DDL)

    def apply(self, db: str, version: int):
        self.history(db, version - 1)
        path = manifest()[version]
        description = path.stem[5:].replace("_", " ").replace("'", "''")
        # SQLx uses SHA-384 of the unmodified file bytes. No stamp after failure:
        # ON_ERROR_STOP closes the session and rolls this entire transaction back.
        self.sql(db, "BEGIN;\n" + path.read_text() + f"""
INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
VALUES ({version}, '{description}', true, decode('{checksum(path)}', 'hex'), 0);
COMMIT;
""")

    def history(self, db: str, last: int) -> list[dict]:
        rows = self.json(db, """
SELECT coalesce(json_agg(row_to_json(h) ORDER BY version), '[]'::json)
FROM (SELECT version, success, encode(checksum, 'hex') AS checksum
      FROM _sqlx_migrations) h
""")
        expected = [{"version": v, "success": True, "checksum": checksum(manifest()[v])}
                    for v in range(1, last + 1)]
        if rows != expected:
            raise AssertionError("Existing SQLx version/checksum history disagrees; no re-stamp allowed")
        return rows

    def schema(self, db: str) -> str:
        from scripts.ci.postgres_migration_contract import CATALOG_SQL, catalog_objects

        catalog = self.scalar(db, CATALOG_SQL)
        try:
            objects = catalog_objects(catalog)
        except ValueError as error:
            raise AssertionError(str(error)) from error
        self.write(f"schema-catalog-{self.sequence:04d}.json", objects)
        return hashlib.sha256(catalog.encode()).hexdigest()

    def columns(self, db: str) -> dict[str, list[str]]:
        return self.json(db, """
SELECT json_object_agg(table_name, columns) FROM (
    SELECT table_name, json_agg(column_name ORDER BY ordinal_position) AS columns
    FROM information_schema.columns WHERE table_schema = 'public'
    GROUP BY table_name
) c
""")

    def fingerprints(self, db: str, columns: dict[str, list[str]]) -> dict:
        # One native query returns counts and one-way row-multiset hashes only.
        # Hash ordering preserves duplicate multiplicity without exposing rows.
        queries = []
        for table, names in sorted(columns.items()):
            if table == "_sqlx_migrations":
                continue
            fields = ", ".join(identifier(name) for name in names)
            relation = identifier(table)
            queries.append(f"SELECT '{table}' AS name,json_build_object('rows',count(*),'sha256',"
                           "encode(sha256(convert_to(coalesce(string_agg(h,',' ORDER BY h),''),'UTF8')),'hex')) AS fingerprint "
                           "FROM (SELECT encode(sha256(convert_to(row_to_json(r)::text,'UTF8')),'hex') AS h "
                           f"FROM (SELECT {fields} FROM {relation}) r) hashes")
        if not queries:
            return {}
        return self.json(db, "SELECT json_object_agg(name,fingerprint) FROM (" + " UNION ALL ".join(queries) + ") f")


    def write(self, name: str, value):
        path = self.output / name
        path.touch(mode=0o600)
        path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")

    def baseline_contract(self, db: str):
        from scripts.rebaseline_sqlx import CONTRACT, catalog_digest_sql

        expected = json.loads(CONTRACT.read_text())["baseline_catalog_sha256"]
        if self.scalar(db, "SET search_path=pg_catalog,public,pg_temp; " + catalog_digest_sql()) != expected:
            raise AssertionError("Fresh baseline differs from the independently generated native catalog")
        self.sql(db, (FIXTURES / "owner_admin_model.sql").read_text())
        self.sql(db, (FIXTURES / "netbox_forward_schema.sql").read_text())

    def conservation(self, db: str) -> dict:
        """Application data and complete ledger hashes, never plaintext rows."""
        sequences = self.json(db, "SELECT coalesce(json_agg(sequence_name ORDER BY sequence_name),'[]') "
                              "FROM information_schema.sequences WHERE sequence_schema='public'")
        return {"catalog": self.schema(db), "rows": self.fingerprints(db, self.columns(db)),
                "history": self.json(db, "SELECT coalesce(json_agg(to_jsonb(h) ORDER BY version),'[]') FROM "
                                     "(SELECT version,description,installed_on,success,encode(checksum,'hex') AS checksum,"
                                     "execution_time FROM _sqlx_migrations) h"),
                "sequences": {name: self.json(db, f"SELECT row_to_json(s) FROM "
                                            f"(SELECT last_value,is_called FROM {identifier(name)}) s")
                              for name in sequences}}

    def maintenance(self, db: str, historical: pathlib.Path | None = None):
        """Exercise real transactions; old data is distinct from factory seeds."""
        from scripts.rebaseline_sqlx import CONTRACT, transition_sql

        if historical is not None:
            expected = json.loads(CONTRACT.read_text())["history"]
            paths = sorted(historical.glob("*.sql"))
            actual = [{"version": int(p.name.split("_", 1)[0]),
                       "description": p.stem[5:].replace("_", " "), "checksum": checksum(p)} for p in paths]
            if actual != expected:
                raise ValueError("Historical proof source differs from the immutable recognized history")
            self.create(db)
            for row, path in zip(expected, paths):
                description = row["description"].replace("'", "''")
                self.sql(db, "BEGIN;\n" + path.read_text() + f"\nINSERT INTO _sqlx_migrations "
                         "(version,description,success,checksum,execution_time) VALUES "
                         f"({row['version']},'{description}',true,decode('{row['checksum']}','hex'),0); COMMIT;")
        else:
            self.create(db)
            self.apply(db, 1)
        self.sql(db, """
INSERT INTO identity_user(id,username,email,auth_source)
VALUES ('00000000-0000-4000-8000-000000000139','maintenance-sentinel','fixture@example.invalid','local');
INSERT INTO auth_user_credential(user_id,password_hash,password_changed_at)
SELECT id,encode(gen_random_bytes(64),'hex'),now() FROM identity_user WHERE username='maintenance-sentinel';
INSERT INTO secrets_variable(id,key) VALUES ('00000000-0000-4000-8000-000000000139','maintenance-sentinel');
INSERT INTO secrets_store(secret_id,encrypted_value)
VALUES ('00000000-0000-4000-8000-000000000139',encode(gen_random_bytes(64),'hex'));
INSERT INTO license_state(instance_id,status,max_seats,entitlement_jwt,entitlement_version,
entitlement_expires_at,activation_token_encrypted)
VALUES ('maintenance-sentinel','active',4,encode(gen_random_bytes(64),'hex'),'fixture-v1',
'2027-01-01T00:00:00Z',encode(gen_random_bytes(64),'hex'));
UPDATE instance_branding SET app_name='Customized installation',theme='{"custom":true}'::jsonb;
""")
        before = self.conservation(db)
        backup = None
        if historical is not None:
            backup = self.command(self.postgres_command("pg_dump", db) + ["-Fc", "--no-owner", "--no-privileges"])
            backup_path = self.output / "pre-transition.dump"
            backup_path.touch(mode=0o600)
            backup_path.write_bytes(backup)
        cases = {
            "partial": "DELETE FROM _sqlx_migrations WHERE version=(SELECT max(version) FROM _sqlx_migrations);",
            "failed": "UPDATE _sqlx_migrations SET success=false WHERE version=1;",
            "foreign": "UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=1;",
            "unknown": "UPDATE _sqlx_migrations SET version=99 WHERE version=1;",
            "renamed": "UPDATE _sqlx_migrations SET description='foreign' WHERE version=1;",
            "schema": "CREATE INDEX maintenance_foreign_index ON identity_user(username);",
            "shadow": "CREATE FUNCTION public.sha256(bytea) RETURNS bytea LANGUAGE SQL AS 'SELECT $1';",
            "trigger": "ALTER TABLE playbook_version DISABLE TRIGGER trg_playbook_version_immutable;",
            "collation": "ALTER COLLATION public.natural_sort RENAME TO foreign_natural_sort;",
        }
        for label, mutation in cases.items():
            clone = db + "_reject"
            self.sql("postgres", f"CREATE DATABASE {identifier(clone)} TEMPLATE {identifier(db)};")
            try:
                self.sql(clone, mutation)
                rejected = self.conservation(clone)
                self.sql(clone, transition_sql(), expected_failure="maintenance refused" if label in
                         ("schema", "failed", "shadow", "trigger", "collation") else "Unrecognized SQLx history")
                if self.conservation(clone) != rejected:
                    raise AssertionError("Refused maintenance changed persistent state")
            finally:
                self.sql("postgres", f"DROP DATABASE {identifier(clone)};")
        if historical is not None:
            role = db + "_role"
            self.sql(db, f"CREATE ROLE {identifier(role)}; GRANT USAGE ON SCHEMA public TO {identifier(role)}; "
                     f"GRANT SELECT,UPDATE,DELETE ON ALL TABLES IN SCHEMA public TO {identifier(role)};")
            try:
                self.sql(db, f"SET ROLE {identifier(role)}; BEGIN; DELETE FROM _sqlx_migrations; ROLLBACK;")
                self.sql(db, f"SET ROLE {identifier(role)};\n" + transition_sql(),
                         expected_failure="permission denied for table _sqlx_migrations")
                if self.conservation(db) != before:
                    raise AssertionError("Failed INSERT did not roll back the preceding ledger DELETE")
            finally:
                self.sql(db, f"DROP OWNED BY {identifier(role)}; DROP ROLE {identifier(role)};")
        self.sql(db, transition_sql())
        after = self.conservation(db)
        if {k: v for k, v in after.items() if k != "history"} != {k: v for k, v in before.items() if k != "history"}:
            raise AssertionError("Maintenance modified application data, schema or sequence state")
        self.history(db, 1)
        self.sql(db, transition_sql())
        if self.conservation(db) != after:
            raise AssertionError("Repeated maintenance was not a no-op")
        if backup is not None:
            restored = db + "_restore"
            self.sql("postgres", f"CREATE DATABASE {identifier(restored)};")
            self.command(self.postgres_command("pg_restore", restored) +
                         ["--exit-on-error", "--no-owner", "--no-privileges"], backup)
            roundtrip = self.conservation(restored)
            # pg_dump reparses native CHECKs and compacts dropped physical slots.
            # The deployment contract owns that schema-roundtrip proof. Here the
            # rollback boundary requires exact old ledger, rows and sequence state.
            if any(roundtrip[key] != before[key] for key in ("rows", "history", "sequences")):
                raise AssertionError("Pre-transition backup restore changed old ledger/data/sequence state")
        self.write("maintenance-" + ("historical" if historical else "baseline") + ".json",
                   {"before": before, "after": after, "refused_cases": list(cases),
                    "atomic_insert_failure": historical is not None, "all_application_data_preserved": True,
                    "old_ledger_backup_restore": backup is not None})


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    transport = parser.add_mutually_exclusive_group(required=True)
    transport.add_argument("--container")
    transport.add_argument("--postgres-host", choices=("postgres",),
                           help="Native psql transport for the isolated CI service")
    parser.add_argument("--output-dir", required=True, type=pathlib.Path)
    parser.add_argument("--prefix", default="owner_proof")
    parser.add_argument("--require-clean-source", action="store_true")
    parser.add_argument("--historical-migrations", type=pathlib.Path,
                        help="Optional immutable source export for the complete-24 transition proof; never a runtime fallback")
    args = parser.parse_args()
    identifier(args.prefix)
    validate_baseline()
    proof = Proof(args.container, args.output_dir, args.require_clean_source, args.postgres_host)
    fresh = args.prefix + "_fresh"
    proof.create(fresh)
    proof.apply(fresh, 1)
    proof.history(fresh, 1)
    proof.baseline_contract(fresh)
    proof.schema(fresh)
    proof.maintenance(args.prefix + "_maintenance")
    if args.historical_migrations:
        proof.maintenance(args.prefix + "_historical", args.historical_migrations)
    if source_provenance(args.require_clean_source) != proof.source:
        raise AssertionError("Source changed during proof; no commit validated")
    proof.write("summary.json", {"postgres": 18, "fresh_baseline": True,
                                 "owner_and_inventory_contracts": True,
                                 "validated_source_sha": proof.source["validated_source_sha"],
                                 "worktree_dirty": proof.source["worktree_dirty"],
                                 "scope": "Fresh native SQL prerequisites, not old-history transition or API/UI acceptance"})
    print("PASS: fresh baseline native catalog and Owner/inventory contracts")
    print(f"Source: validated_sha={proof.source['validated_source_sha']}; dirty={proof.source['worktree_dirty']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
