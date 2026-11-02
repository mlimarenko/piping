#!/usr/bin/env python3
"""Offline transition of the exact complete pre-1.0 SQLx history.

Never runs factory SQL or modifies application rows. Stop all writers and keep
a verified private backup. Old images require restoring that backup afterward.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from scripts.ci.postgres_migration_contract import CATALOG_SQL

CONTRACT = ROOT / "apps/api/migrations/maintenance-transition.json"
BASELINE = ROOT / "apps/api/migrations/0001_init.sql"


def catalog_digest_sql() -> str:
    """Bind native schema, excluding only server-generated ICU version metadata.

    Deployment still compares the complete catalog. Maintenance separately
    refuses stale collation versions; a different ICU build is not schema drift.
    """
    catalog = CATALOG_SQL.strip().removesuffix(";")
    return f"""SELECT encode(sha256(convert_to(jsonb_agg(
        CASE WHEN obj->>'kind' = 'collation'
             THEN jsonb_set(obj,'{{definition}}',(obj->'definition') - 'version')
             ELSE obj END ORDER BY obj->>'kind' COLLATE "C", obj->>'name' COLLATE "C")::text,
        'UTF8')),'hex') FROM jsonb_array_elements(({catalog})) obj"""


def installed_catalog_digest_sql(contract: dict) -> str:
    """Recognize one complete installed layout without changing its native objects.

    The projection is bounded to source-reviewed column identities/positions,
    one exact CHECK representation and the preserved pre-semantic ledger. The
    resulting entire catalog must still match the sealed historical catalog.
    """
    layout = contract["installed_layout"]
    columns = {r["name"]: {"from": r["from"], "to": r["to"]}
               for r in layout["columns"]}
    constraints = {r["name"]: {"from": r["from"], "to": r["to"]}
                   for r in layout["constraints"]}
    if len(columns) != len(layout["columns"]) or len(constraints) != len(layout["constraints"]):
        raise ValueError("Duplicate installed-layout object identity")
    if any(type(v[k]) is not int or v[k] < 1 for v in columns.values() for k in ("from", "to")):
        raise ValueError("Invalid installed-layout column position")
    if layout["provenance"]["reference_catalog_sha256"] != contract["historical_catalog_sha256"]:
        raise ValueError("Installed layout must bind the historical source catalog")

    def literal(value: object) -> str:
        return "'" + json.dumps(value, separators=(",", ":")).replace("'", "''") + "'::jsonb"

    catalog = CATALOG_SQL.strip().removesuffix(";")
    return f"""WITH observed AS (SELECT ({catalog}) AS objects),
    source_contract AS (SELECT {literal(columns)} AS columns,
        {literal(constraints)} AS constraints,
        {literal(layout['archive_objects'])} AS archive_objects,
        {literal(layout['archive_history'])} AS archive_history),
    qualified AS (
        SELECT objects, columns, constraints, archive_objects FROM observed, source_contract
        WHERE NOT EXISTS (
            SELECT FROM jsonb_each(columns) m WHERE
            (SELECT obj->'definition'->'position' FROM jsonb_array_elements(objects) obj
             WHERE obj->>'kind'='column' AND obj->>'name'=m.key)
                IS DISTINCT FROM m.value->'from')
        AND NOT EXISTS (
            SELECT FROM jsonb_each(constraints) m WHERE
            (SELECT obj->'definition' FROM jsonb_array_elements(objects) obj
             WHERE obj->>'kind'='constraint' AND obj->>'name'=m.key)
                IS DISTINCT FROM m.value->'from')
        AND (SELECT coalesce(jsonb_agg(obj ORDER BY obj->>'kind' COLLATE "C",
                obj->>'name' COLLATE "C"),'[]'::jsonb)
             FROM jsonb_array_elements(objects) obj
             WHERE EXISTS (SELECT FROM jsonb_array_elements(archive_objects) a
                WHERE a->>'kind'=obj->>'kind' AND a->>'name'=obj->>'name')) = archive_objects
        AND (SELECT coalesce(jsonb_agg(jsonb_build_object(
                'version',version,'description',description,'success',success,
                'checksum',encode(checksum,'hex')) ORDER BY version),'[]'::jsonb)
             FROM public._sqlx_migrations_pre_semantic_20260719) = archive_history
        AND NOT EXISTS (SELECT FROM public._sqlx_migrations_pre_semantic_20260719
            WHERE NOT success OR execution_time < 0 OR NOT isfinite(installed_on))
    ), projected AS (
        SELECT CASE
          WHEN obj->>'kind'='column' AND columns ? (obj->>'name') THEN
            jsonb_set(obj,'{{definition,position}}',columns->(obj->>'name')->'to')
          WHEN obj->>'kind'='constraint' AND constraints ? (obj->>'name') THEN
            jsonb_set(obj,'{{definition}}',constraints->(obj->>'name')->'to')
          WHEN obj->>'kind'='collation' THEN
            jsonb_set(obj,'{{definition}}',(obj->'definition') - 'version')
          ELSE obj END AS obj
        FROM qualified, jsonb_array_elements(objects) obj
        WHERE NOT EXISTS (SELECT FROM jsonb_array_elements(archive_objects) a
            WHERE a->>'kind'=obj->>'kind' AND a->>'name'=obj->>'name')
    )
    SELECT encode(sha256(convert_to(jsonb_agg(obj ORDER BY obj->>'kind' COLLATE "C",
        obj->>'name' COLLATE "C")::text,'UTF8')),'hex') FROM projected"""


def transition_sql() -> str:
    """Check recognized history and native schema before the sole persistent write."""
    contract = json.loads(CONTRACT.read_text())
    history = contract["history"]
    if (contract["schema"] != "piping.sqlx-maintenance/v1"
            or [r["version"] for r in history] != list(range(1, 25))):
        raise ValueError("Unsupported maintenance history")
    for row in history:
        if (set(row) != {"version", "description", "checksum"}
                or not isinstance(row["description"], str)
                or len(bytes.fromhex(row["checksum"])) != 48):
            raise ValueError("Invalid maintenance history identity")
    for key in ("historical_catalog_sha256", "baseline_catalog_sha256"):
        if len(bytes.fromhex(contract[key])) != 32:
            raise ValueError("Invalid native catalog digest")
    checksum = hashlib.sha384(BASELINE.read_bytes()).hexdigest()
    expected = json.dumps(history, separators=(",", ":")).replace("'", "''")
    catalog = catalog_digest_sql()
    installed_catalog = installed_catalog_digest_sql(contract)
    return f"""\\set ON_ERROR_STOP on
BEGIN;
SET LOCAL lock_timeout = '10s';
SET LOCAL statement_timeout = '120s';
SET LOCAL search_path = pg_catalog, public, pg_temp;
DO $maintenance$
DECLARE
    observed jsonb;
    native_digest text;
    expected_digest text;
    relation record;
BEGIN
    IF to_regclass('public._sqlx_migrations') IS NULL THEN
        RAISE EXCEPTION 'SQLx history is absent; maintenance requires a recognized installation';
    END IF;
    IF current_setting('server_version_num')::int / 10000 <> 18 OR EXISTS (
        SELECT FROM pg_collation c JOIN pg_namespace n ON n.oid=c.collnamespace
        WHERE n.nspname NOT LIKE 'pg\\_%' ESCAPE '\\'
          AND c.collversion IS DISTINCT FROM pg_collation_actual_version(c.oid)
    ) THEN
        RAISE EXCEPTION 'Require PostgreSQL 18 and current native collation versions';
    END IF;
    -- All writers must remain stopped, including after COMMIT. Acquire data/DDL
    -- locks before observing history and catalog, without changing application data.
    FOR relation IN
        SELECT n.nspname, c.relname FROM pg_class c
        JOIN pg_namespace n ON n.oid=c.relnamespace
        WHERE c.relkind IN ('r','p') AND n.nspname NOT LIKE 'pg\\_%' ESCAPE '\\'
          AND n.nspname <> 'information_schema'
        ORDER BY n.nspname COLLATE "C", c.relname COLLATE "C"
    LOOP
        EXECUTE format('LOCK TABLE %I.%I IN ACCESS EXCLUSIVE MODE', relation.nspname, relation.relname);
    END LOOP;
    SELECT coalesce(jsonb_agg(jsonb_build_object('version',version,'description',description,
               'checksum',encode(checksum,'hex')) ORDER BY version),'[]'::jsonb)
      INTO observed FROM public._sqlx_migrations;
    IF EXISTS (SELECT FROM public._sqlx_migrations WHERE NOT success OR execution_time < 0
               OR NOT isfinite(installed_on)) THEN
        RAISE EXCEPTION 'Failed or invalid SQLx history; maintenance refused';
    END IF;
    IF observed = '[{{"version":1,"description":"init","checksum":"{checksum}"}}]'::jsonb THEN
        expected_digest := '{contract['baseline_catalog_sha256']}';
    ELSIF observed = '{expected}'::jsonb THEN
        expected_digest := '{contract['historical_catalog_sha256']}';
    ELSE
        RAISE EXCEPTION 'Unrecognized SQLx history; only the exact complete 24 history is supported';
    END IF;
    SELECT ({catalog}) INTO native_digest;
    IF native_digest <> expected_digest
       AND to_regclass('public._sqlx_migrations_pre_semantic_20260719') IS NOT NULL THEN
        -- Locks above cover both ledgers and every application table. Only the
        -- recognized projection may differ; no catalog or archive row is edited.
        SELECT ({installed_catalog}) INTO native_digest;
    END IF;
    IF native_digest IS NULL OR (native_digest <> expected_digest AND NOT (
        jsonb_array_length(observed) = 1
        AND native_digest = '{contract['historical_catalog_sha256']}'
    )) THEN
        RAISE EXCEPTION 'Native schema differs from the recognized history; maintenance refused';
    END IF;
    IF jsonb_array_length(observed) = 1 THEN
        RETURN;
    END IF;
    DELETE FROM public._sqlx_migrations;
    INSERT INTO public._sqlx_migrations
        (version,description,installed_on,success,checksum,execution_time)
        VALUES (1,'init',now(),true,decode('{checksum}','hex'),0);
END
$maintenance$;
COMMIT;
"""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--maintenance-confirmed", action="store_true",
                        help="All writers are stopped until the consolidated-baseline backend starts")
    parser.add_argument("--backup-confirmed", action="store_true",
                        help="A verified private pre-transition backup and old image identity are retained")
    parser.add_argument("command", choices=("print-sql", "apply-local-compose", "apply-compose", "apply-env"))
    parser.add_argument("compose_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if not args.maintenance_confirmed or not args.backup_confirmed:
        parser.error("Require --maintenance-confirmed and --backup-confirmed; not an automatic repair")
    sql = transition_sql().encode()
    if args.command == "print-sql":
        sys.stdout.buffer.write(sql)
        return 0
    if args.command == "apply-env":
        if args.compose_args:
            parser.error("apply-env accepts libpq environment only, not connection arguments")
        # libpq consumes PGHOST/PGPORT/PGDATABASE/PGUSER/PGPASSFILE. Never put
        # credentials in arguments or echo database diagnostics to the terminal.
        command = ["psql", "-X", "-q", "-v", "ON_ERROR_STOP=1"]
    else:
        compose = args.compose_args
        if compose[:1] == ["--"]:
            compose = compose[1:]
        if args.command == "apply-local-compose":
            if compose:
                parser.error("apply-local-compose does not accept Compose arguments")
            compose = ["-f", str(ROOT / "docker-compose.yml"), "--env-file", str(ROOT / "env/dev.env")]
        elif not compose:
            parser.error("apply-compose requires explicit Compose arguments after --")
        command = ["docker", "compose", *compose, "exec", "-T", "postgres", "sh", "-eu", "-c",
                   'exec psql -X -q -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB"']
    result = subprocess.run(command, input=sql, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            env=os.environ.copy(), timeout=180, check=False)
    if result.returncode:
        print("Maintenance failed; transaction not accepted. Keep writers stopped; inspect privately.", file=sys.stderr)
    else:
        print("Maintenance committed; start only the baseline backend. Old images require private restore.")
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
