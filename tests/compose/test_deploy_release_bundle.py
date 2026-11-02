"""Exercise archive-backed deployment against isolated native Docker resources.

No Docker-unavailable skip: this discovery route requires a working engine.
Every resource has a per-run project name; cleanup never targets the host stack.
scripts/ci-compose-native.py places this client in the daemon's network namespace.
"""

from __future__ import annotations

import copy
from contextlib import contextmanager
import argparse
import base64
import hashlib
import json
import os
import re
import runpy
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import unittest
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from unittest.mock import patch

from scripts import deploy_release_bundle as deploy
from scripts.ci import postgres_migration_contract as schema_contract
from scripts.ci.compose_delivery_contract import package_binding

ROOT = Path(__file__).resolve().parents[2]
DEPLOY = ROOT / "scripts/deploy_release_bundle.py"
COMMIT = "c" * 40
SCHEDULE = "00000000-0000-4000-8000-000000000097"
KEYS = ("backend", "frontend", "ansible-runner")
SYNTHETIC_CIPHER_KEY = base64.b64encode(bytes(range(32))).decode()
PRODUCER = runpy.run_path(str(ROOT / "scripts/ci-compose-native.py"))


class NativeEdgeRefreshTests(unittest.TestCase):
    """Refresh the actual Compose-owned edge without replacing customer ingress."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="edge-refresh-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.project = "edge-refresh-" + uuid.uuid4().hex
        self.config = self.root / "nginx.conf"
        self.configure("before")
        (self.root / ".env").write_text("")
        self.compose_file = self.root / "docker-compose.yml"
        self.compose_file.write_text(json.dumps({"services": {
            "nginx": {
                "image": "nginx:1.30-alpine",
                "volumes": [str(self.config) + ":/etc/nginx/nginx.conf:ro"],
            },
            "frontend": {"image": "nginx:1.30-alpine"},
        }}))
        self.deployment = deploy.Deployment(argparse.Namespace(
            root=self.root, stage=self.root / "backups/release-20261025T120000Z",
            project_name=self.project))
        self.deployment.plan = {"image_file": ".env"}
        deploy.run(self.deployment.compose() + ["up", "-d", "--pull", "never"])
        self.identifier = deploy.run(self.deployment.compose() + ["ps", "-q", "nginx"]).decode().strip()
        self.network = self.project + "_default"
        self.addCleanup(self.remove_network)
        self.before = self.inspect()

    def remove_network(self):
        identifiers = deploy.run(["docker", "ps", "-aq", "--filter",
                                  "label=com.docker.compose.project=" + self.project]).decode().splitlines()
        if identifiers:
            deploy.run(["docker", "rm", "-f", *identifiers])
        deploy.run(["docker", "network", "rm", self.network])

    def configure(self, body):
        self.config.write_text('events {}\nhttp { server { listen 80; location / { return 200 "'
                               + body + '"; } } }\n')

    def inspect(self):
        return json.loads(deploy.run(["docker", "inspect", self.identifier]))[0]

    def test_refreshes_declared_and_retained_edge_in_place(self):
        for declared in (True, False):
            with self.subTest(declared=declared):
                if not declared:
                    self.compose_file.write_text(json.dumps({"services": {
                        "frontend": {"image": "nginx:1.30-alpine"}}}))
                self.configure("after-" + str(declared))
                config_bytes = self.config.read_bytes()
                with patch.object(deploy, "run", wraps=deploy.run) as commands:
                    self.deployment.refresh_edge_upstreams()
                executions = [call.args[0] for call in commands.call_args_list
                              if call.args[0][:2] == ["docker", "exec"]]
                self.assertEqual(executions, [
                    ["docker", "exec", self.identifier, "nginx", "-t"],
                    ["docker", "exec", self.identifier, "nginx", "-s", "reload"],
                ])
                services = self.deployment.running_declared_services()
                self.assertEqual(services, ["frontend", "nginx"] if declared else ["frontend"])
                state = self.inspect()
                self.assertEqual(state["Id"], self.before["Id"])
                self.assertEqual(state["State"]["StartedAt"], self.before["State"]["StartedAt"])
                self.assertTrue(state["State"]["Running"])
                self.assertEqual(self.config.read_bytes(), config_bytes)
                self.assertEqual(deploy.run(["docker", "exec", self.identifier, "nginx", "-t"]), b"")

    def test_refuses_ambiguous_edge(self):
        deploy.run(["docker", "run", "-d", "--pull", "never",
                    "--label", "com.docker.compose.project=" + self.project,
                    "--label", "com.docker.compose.service=nginx",
                    "nginx:1.30-alpine"])
        with self.assertRaisesRegex(deploy.DeployError, "unique"):
            self.deployment.refresh_edge_upstreams()

    def test_refuses_stopped_edge(self):
        deploy.run(["docker", "stop", self.identifier])
        with self.assertRaises(deploy.DeployError):
            self.deployment.refresh_edge_upstreams()

    def test_edgeless_installation_needs_no_refresh(self):
        deploy.run(["docker", "rm", "-f", self.identifier])
        self.compose_file.write_text(json.dumps({"services": {
            "frontend": {"image": "nginx:1.30-alpine"}}}))
        with patch.object(deploy, "run", wraps=deploy.run) as commands:
            self.deployment.refresh_edge_upstreams()
        self.assertFalse(any(call.args[0][:2] == ["docker", "exec"]
                             for call in commands.call_args_list))

    def test_declared_missing_edge_cannot_skip_refresh(self):
        deploy.run(["docker", "rm", "-f", self.identifier])
        with self.assertRaisesRegex(deploy.DeployError, "declared nginx container is missing"):
            self.deployment.refresh_edge_upstreams()

    def test_invalid_config_cannot_reload(self):
        self.config.write_text("invalid customer configuration;\n")
        with patch.object(deploy, "run", wraps=deploy.run) as commands:
            with self.assertRaises(deploy.DeployError):
                self.deployment.refresh_edge_upstreams()
        self.assertFalse(any(call.args[0][-3:] == ["nginx", "-s", "reload"]
                             for call in commands.call_args_list))
        self.assertEqual(self.inspect()["State"]["StartedAt"], self.before["State"]["StartedAt"])
        self.assertEqual(self.config.read_text(), "invalid customer configuration;\n")

    def test_refuses_inspection_identity_change_before_exec(self):
        native_run = deploy.run
        for change in ("id", "project", "service", "oneoff"):
            with self.subTest(change=change):
                def inspect_changed(command, **kwargs):
                    result = native_run(command, **kwargs)
                    if command[:2] == ["docker", "inspect"]:
                        inspected = json.loads(result)
                        if change == "id":
                            inspected[0]["Id"] = "f" * 64
                        else:
                            key = "com.docker.compose." + change
                            inspected[0]["Config"]["Labels"][key] = "True" if change == "oneoff" else "foreign"
                        return json.dumps(inspected).encode()
                    return result

                with patch.object(deploy, "run", side_effect=inspect_changed) as commands:
                    with self.assertRaisesRegex(deploy.DeployError, "identity"):
                        self.deployment.refresh_edge_upstreams()
                self.assertFalse(any(call.args[0][:2] == ["docker", "exec"]
                                     for call in commands.call_args_list))


class NativeMigrationContractTests(unittest.TestCase):
    """Real backend SQLx entrypoint; maintenance has a separate native proof."""

    def test_native_baseline_nonempty_replay_and_history_refusal(self):
        binary = Path(os.environ["PIPING_NATIVE_BACKEND_BINARY"])
        migrations = Path(os.environ["PIPING_NATIVE_MIGRATIONS_DIR"])
        self.assertEqual(deploy.file_sha(binary), os.environ["PIPING_NATIVE_BACKEND_SHA256"])
        self.assertEqual(deploy.file_sha(migrations / "0001_init.sql"),
                         os.environ["PIPING_NATIVE_BASELINE_SHA256"])
        artifact_root = Path(os.environ.get("PIPING_NATIVE_ARTIFACT_ROOT", "/data/artifacts/PIP-139-native"))
        artifact_root.mkdir(parents=True, exist_ok=True)
        root = Path(tempfile.mkdtemp(prefix="baseline-contract-", dir=artifact_root))
        root.chmod(0o700)
        (root / "backups").mkdir(mode=0o700)
        stage = root / "backups" / "release-20261021T120000Z"
        stage.mkdir(mode=0o700)
        deployment = deploy.Deployment(argparse.Namespace(root=root, stage=stage, project_name="native-contract"))
        checksums = {str(int(p.name.split("_", 1)[0])): hashlib.sha384(p.read_bytes()).hexdigest()
                     for p in sorted(migrations.glob("*.sql"))}
        contract = json.loads((ROOT / "apps/api/migrations/compose-deployment.json").read_bytes())
        deploy.validate_contract(contract, checksums)
        deployment.plan = {"migrations": checksums, "contract": contract,
                           "prefix_manifest_sha384": {"apps/api/migrations/" + p.name: checksums[str(int(p.name.split("_", 1)[0]))]
                                                       for p in migrations.glob("*.sql")},
                            "postgres_image": json.loads(deploy.run(["docker", "image", "inspect", "postgres:18-trixie"]))[0]["Id"]}
        deploy.write(stage / "proof-backend.env", (
            "PIPING_SECRETS__CIPHER_KEY_B64=" + SYNTHETIC_CIPHER_KEY + "\n"
            "PIPING_AUTH__PASSWORD_PEPPER=" + uuid.uuid4().hex + "\n"
            "PIPING_AUTH__LOCAL_ENABLED=false\nPIPING_OBSERVABILITY__OTEL_ENABLED=false\n"
            "PIPING_REDIS__URL=redis://127.0.0.1:1\n").encode())
        with deployment.isolated_database("native") as database:
            container = deploy.run(database["compose"] + ["ps", "-q", "postgres"]).decode().strip()
            deploy.run(["docker", "exec", container, "mkdir", "-p", "/proof/config"])
            for source, target in ((binary, "/proof/piping-backend"), (migrations, "/proof/migrations"),
                                   (ROOT / "apps/api/config/default.toml", "/proof/config/default.toml")):
                deploy.run(["docker", "cp", str(source), container + ":" + target], timeout=300)

            def migrate(*, refused=False):
                local = {**database, "url": database["url"].replace("@postgres:", "@127.0.0.1:")}
                with deployment.isolated_environment(local, start_mode="migrate") as env_file, \
                        (root / ("rejected.log" if refused else "migrate.log")).open("ab") as output:
                    command = ["docker", "exec", "--env-file", str(env_file), "-w", "/proof",
                               "-e", "PIPING_MIGRATIONS_DIR=/proof/migrations", container, "/proof/piping-backend"]
                    result = subprocess.run(command, stdout=output, stderr=output, timeout=300,
                                            env=deploy.environment(), check=False)
                self.assertEqual(result.returncode != 0, refused, "See private native migration logs")

            migrate()
            with deployment.database_target(database):
                self.assertEqual(len(deployment.guard_history(completed=True)), len(checksums))
                catalog = deployment.catalog()
                self.assertTrue(any(o["kind"] == "collation" and o["name"] == "public.natural_sort" for o in catalog))
                self.assertTrue(any(o["kind"] == "trigger" and o["definition"]["internal"] for o in catalog))
                deployment.sql("UPDATE instance_branding SET app_name='Customized synthetic installation';")
                before = deployment.fingerprint()
            migrate()
            with deployment.database_target(database):
                self.assertEqual(deployment.fingerprint(), before)
                deployment.sql("UPDATE _sqlx_migrations SET version=99 WHERE version=1;")
                rejected = deployment.fingerprint()
            migrate(refused=True)
            with deployment.database_target(database):
                self.assertEqual(deployment.fingerprint(), rejected)
            deploy.write_json(root / "baseline-migration-receipt.json", {
                "source_sha": os.environ.get("PIPING_NATIVE_SOURCE_SHA"),
                "backend_sha256": os.environ["PIPING_NATIVE_BACKEND_SHA256"],
                "baseline_sha256": os.environ["PIPING_NATIVE_BASELINE_SHA256"],
                "fresh_baseline": True, "nonempty_replay_unchanged": True,
                "foreign_history_refused_unchanged": True})



class NativeForwardMigrationContractTests(unittest.TestCase):
    """Synthetic R -> C -> DDL rehearsals, never evidence of installed readiness."""

    GRAPH = ("secrets_variable", "secrets_store", "secrets_binding", "secret_usage_binding")
    CONTRACT_PATH = ROOT / "apps/api/migrations/compose-deployment.json"

    def test_fresh_prefix_upgrade_and_fail_closed_history(self):
        binary = Path(os.environ["PIPING_NATIVE_BACKEND_BINARY"])
        converter = Path(os.environ["PIPING_NATIVE_CONVERTER_BINARY"])
        migrations = Path(os.environ["PIPING_NATIVE_MIGRATIONS_DIR"])
        identities = {
            "source_sha": os.environ["PIPING_NATIVE_SOURCE_SHA"],
            "backend_image_id": os.environ["PIPING_NATIVE_BACKEND_IMAGE_ID"],
            "backend_sha256": os.environ["PIPING_NATIVE_BACKEND_SHA256"],
            "converter_sha256": os.environ["PIPING_NATIVE_CONVERTER_SHA256"],
            "helper_sha256": deploy.helper_identity(),
            "baseline_sha256": os.environ["PIPING_NATIVE_BASELINE_SHA256"],
        }
        self.assertTrue(deploy.file_sha(binary) == identities["backend_sha256"], "backend identity mismatch")
        self.assertTrue(deploy.file_sha(converter) == identities["converter_sha256"], "converter identity mismatch")
        self.assertTrue(deploy.file_sha(migrations / "0001_init.sql") == identities["baseline_sha256"],
                        "baseline identity mismatch")
        migration_files = sorted(migrations.glob("*.sql"))
        self.native_failure_context = {"provided_count": len(migration_files)}
        history = schema_contract.source_migration_history(
            (path.name, path.read_bytes()) for path in migration_files)
        self.native_failure_context["actual_versions"] = [row["version"] for row in history]
        self.assertEqual(sorted(row["name"] for row in history), [path.name for path in migration_files])
        self.assertGreaterEqual(len(history), 4)
        self.assertEqual([row["version"] for row in history], list(range(1, len(migration_files) + 1)))
        del self.native_failure_context
        prefix = {"apps/api/migrations/" + row["name"]: row["checksum"] for row in history}
        self.assertEqual(prefix[schema_contract.SERVICE_CUTOVER_PATH], schema_contract.SERVICE_CUTOVER_SHA384)
        self.assertEqual(prefix[schema_contract.FHRP_CUTOVER_PATH], schema_contract.FHRP_CUTOVER_SHA384)
        self.assertGreater(len(history), 6)
        self.assertTrue(schema_contract.content_type_seeds_pending(prefix, 6))
        self.assertEqual(schema_contract.pending_transforms(prefix, 6), ("ContentTypeSeeds",))
        self.assertFalse(schema_contract.content_type_seeds_pending(prefix, len(history)))
        substituted = dict(prefix)
        seed_path = schema_contract.CONTENT_TYPE_SEED_MIGRATIONS[8][0]
        substituted[seed_path] = "0" * 96
        with self.assertRaises(ValueError):
            schema_contract.content_type_seeds_pending(substituted, 6)
        missing = dict(prefix)
        del missing[seed_path]
        with self.assertRaises(ValueError):
            schema_contract.content_type_seeds_pending(missing, 6)
        aliased = dict(prefix)
        aliased[seed_path.replace("0008_", "008_")] = aliased[seed_path]
        with self.assertRaises(ValueError):
            schema_contract.content_type_seeds_pending(aliased, 6)
        base_image = json.loads(deploy.run(["docker", "image", "inspect", identities["backend_image_id"]]))[0]
        self.assertEqual(base_image["Id"], identities["backend_image_id"])
        self.assertEqual(base_image["Config"]["Labels"]["org.opencontainers.image.revision"], identities["source_sha"])
        artifact_root = Path(os.environ["PIPING_NATIVE_ARTIFACT_ROOT"])
        root = Path(tempfile.mkdtemp(prefix="genuine-fhrp-chain-", dir=artifact_root))
        root.chmod(0o700)
        self.enterContext(patch.object(deploy, "COMMAND_DIAGNOSTICS", root))
        # BuildKit parses a bare sha256 image ID in FROM as a registry name.
        # Give the already verified local image an owned reference, retaining its
        # immutable ID as authority rather than resolving any registry fallback.
        base_reference = "pwa82-chain-base-" + uuid.uuid4().hex + ":owned"
        owned_base = {}
        self.addCleanup(self.remove_chain_images, owned_base)
        deploy.run(["docker", "image", "tag", identities["backend_image_id"], base_reference])
        owned_base["base"] = base_reference
        self.assertEqual(json.loads(deploy.run(["docker", "image", "inspect", base_reference]))[0]["Id"],
                         identities["backend_image_id"])
        images = {}
        self.addCleanup(self.remove_chain_images, images)
        # Every image carries the verified executable pair and exact SQL bytes.
        # Prefix selection is SQLx's migration directory, not a manufactured ledger.
        for installed in dict.fromkeys((1, 2, 3, 4, 6, len(history))):
            context = root / ("image-prefix-" + str(installed))
            (context / "migrations").mkdir(parents=True)
            (context / "config").mkdir()
            shutil.copy2(self.CONTRACT_PATH, context / "migrations/compose-deployment.json")
            for row in history[:installed]:
                shutil.copy2(migrations / row["name"], context / "migrations" / row["name"])
            shutil.copy2(binary, context / "piping-backend")
            shutil.copy2(converter, context / "convert-fhrp-auth-keys")
            shutil.copy2(ROOT / "apps/api/config/default.toml", context / "config/default.toml")
            (context / "Dockerfile").write_text(
                "FROM " + base_reference + "\n"
                "USER root\nRUN rm -rf /app/migrations\n"
                "COPY migrations /app/migrations\nCOPY config /app/config\n"
                "COPY piping-backend /usr/local/bin/piping-backend\n"
                "COPY convert-fhrp-auth-keys /usr/local/bin/convert-fhrp-auth-keys\n"
                "WORKDIR /app\nENTRYPOINT [\"/usr/local/bin/piping-backend\"]\n")
            tag = "pwa82-chain-" + uuid.uuid4().hex + ":prefix" + str(installed)
            self.assertEqual(json.loads(deploy.run(["docker", "image", "inspect", base_reference]))[0]["Id"],
                             identities["backend_image_id"])
            deploy.run(["docker", "build", "--pull=false", "--network=none", "-t", tag, str(context)], timeout=300)
            images[installed] = tag
            executable_digests = deploy.run([
                "docker", "run", "--rm", "--pull", "never", "--network", "none", "--entrypoint", "sha256sum", tag,
                "/usr/local/bin/piping-backend", "/usr/local/bin/convert-fhrp-auth-keys"]).decode().splitlines()
            self.assertEqual(executable_digests, [
                identities["backend_sha256"] + "  /usr/local/bin/piping-backend",
                identities["converter_sha256"] + "  /usr/local/bin/convert-fhrp-auth-keys"])
        cohorts = []
        for installed in (1, 2, 3):
            # Do not suppress a failed cohort with subTest and then emit success.
            cohorts.append(self.prove_prepared_chain(
                root, installed, images, converter, identities, history, prefix, base_reference))
        self.assertEqual([cohort["installed_prefix"] for cohort in cohorts], [1, 2, 3])
        self.prove_content_type_chain(root, images, identities, history, prefix)
        self.remove_chain_images(images)
        self.remove_chain_images(owned_base)
        # No success receipt exists until all three cohorts and every negative pass.
        deploy.write_json(root / "fhrp-conversion-chain-receipt.json", {
            "schema": "piping.fhrp-conversion-chain/v1", **identities, "cohorts": cohorts,
            "original_r_restored": True, "canonical_converter_executed": True,
            "effective_backend_cipher_environment": True, "prepared_c_restored": True,
            "old_secret_graph_and_ciphertext_conserved": True, "c_to_ddl_expectations_verified": True})
        deploy.write_json(root / "forward-migration-receipt.json", {
            "source_sha": identities["source_sha"], "backend_sha256": identities["backend_sha256"],
            "baseline_sha256": identities["baseline_sha256"], "fresh_and_replay": True,
            "prefixes": [1, 2, 3], "native_ledger_metadata_preserved": True,
            "old_rows_and_append_positions_preserved": True, "failed_sql_rolled_back": True,
            "destructive_file_without_authorized_transform_rejected": True,
            "history_rejections": ["unknown", "failed", "checksum", "identity", "gap", "reordered"],
            "destructive_catalog_rejections": ["removed", "changed"]})
        deploy.write_json(root / "service-cutover-receipt.json", {
            "source_sha": identities["source_sha"], "helper_sha256": identities["helper_sha256"],
            "backend_sha256": identities["backend_sha256"], "baseline_sha256": identities["baseline_sha256"],
            "migration_sha384": schema_contract.SERVICE_CUTOVER_SHA384,
            "all_rows_and_ip_graph_preserved": True, "restore_rederived_expectation": True,
            "raw_fingerprint_retained": True, "unexpected_removal_rejected": True,
            "corrupt_values_and_edges_rejected": True, "expectation_tamper_rejected": True,
            "invalid_source_rejected": True, "canonical_replay_unchanged": True})
        deploy.write_json(root / "content-type-seeds-receipt.json", {
            "schema": "piping.native-content-type-seeds/v1", **identities,
            "installed_prefix": 6, "original_ids_and_rows_preserved": True,
            "required_keys_and_exact_residual": True, "restored_expectation_rederived": True,
            "actual_ids_stable_on_replay": True, "row_mutations_rejected": True,
            "expectation_tamper_rejected": True, "source_substitution_rejected": True,
            "unapproved_source_write_rejected": True})

    @staticmethod
    def remove_chain_images(images):
        for key, image in list(images.items()):
            deploy.run(["docker", "image", "rm", image])
            del images[key]

    def prove_content_type_chain(self, root, images, identities, history, prefix):
        """Exercise registry-only conservation after all earlier cutovers completed."""
        cohort = root / "prefix-6-registry"
        stage = cohort / "backups/release-20261027T120000Z"
        stage.mkdir(parents=True, mode=0o700)
        cohort.chmod(0o700)
        owner = deploy.Deployment(argparse.Namespace(
            root=cohort, stage=stage, project_name="pip171-registry-" + uuid.uuid4().hex))
        image_ids = {n: json.loads(deploy.run(["docker", "image", "inspect", images[n]]))[0]["Id"]
                     for n in (6, len(history))}
        owner.plan = {
            "migrations": {str(row["version"]): row["checksum"] for row in history},
            "contract": json.loads(self.CONTRACT_PATH.read_bytes()), "prefix_manifest_sha384": prefix,
            "commit": identities["source_sha"], "platform": "linux/amd64",
            "disabled_schedule": None, "preservation_sha256": None,
            "postgres_image": os.environ.get("PIPING_NATIVE_POSTGRES_IMAGE", "postgres:18-trixie"),
            "images": {"backend": {"local_id": image_ids[len(history)]}},
            "installed_backend_image": image_ids[6],
        }
        owner.published_migrations()
        deploy.write(stage / "proof-backend.env", (
            "PIPING_SECRETS__CIPHER_KEY_B64=" + base64.b64encode(os.urandom(32)).decode() + "\n"
            "PIPING_AUTH__PASSWORD_PEPPER=" + uuid.uuid4().hex + "\n"
            "PIPING_AUTH__LOCAL_ENABLED=false\nPIPING_OBSERVABILITY__OTEL_ENABLED=false\n"
            "PIPING_REDIS__URL=redis://127.0.0.1:1\n").encode())
        literal = lambda value: "'" + value.replace("'", "''") + "'"
        with owner.isolated_database("registry-prefix-six") as original:
            def create(label):
                name = "registry_" + label + "_" + uuid.uuid4().hex[:12]
                with owner.database_target(original):
                    owner.sql("CREATE DATABASE " + name + ";")
                return {**original, "name": name, "url": original["url"].rsplit("/", 1)[0] + "/" + name}

            def fingerprint(database):
                with owner.database_target(database):
                    return owner.fingerprint()

            def dump(database, path):
                with path.open("xb") as output:
                    path.chmod(0o600)
                    deploy.run(database["compose"] + ["exec", "-T", "postgres", "pg_dump", "-U", "piping",
                               "-d", database.get("name", "piping"), "-Fc", "--no-owner", "--no-privileges"],
                               output=output, timeout=300)

            owner.isolated_migrate(original, image_ids[6])
            canonical = fingerprint(original)
            owner.plan["installed_history"] = canonical["history"]
            self.assertEqual(len(canonical["history"]), 6)
            self.assertEqual(owner.transforms(), ("ContentTypeSeeds",))
            keys = owner.native_content_type_seed_keys(original)
            owner.plan["content_type_seed_keys"] = keys

            # A checksum-consistent but unapproved future file cannot authorize
            # registry writes merely because the native producer observes them.
            extra_name = f"{len(history) + 1:04d}_unapproved_registry.sql"
            extra_key = "apps/api/migrations/" + extra_name
            extra_sql = b"INSERT INTO inventory_content_type(app_label,model) VALUES ('pip171_unapproved','write');"
            extra_path = stage / "source-migrations" / extra_name
            deploy.write(extra_path, extra_sql)
            owner.plan["prefix_manifest_sha384"] = {**prefix, extra_key: hashlib.sha384(extra_sql).hexdigest()}
            try:
                with patch.object(owner, "sql", wraps=owner.sql) as executed_sql:
                    with self.assertRaisesRegex(ValueError, "^Unknown checksum-bound content-type source migration$"):
                        owner.native_content_type_seed_keys(original)
                    executed_sql.assert_not_called()
                with owner.database_target(original):
                    self.assertEqual(owner.sql("SELECT count(*) FROM public.inventory_content_type "
                                               "WHERE app_label='pip171_unapproved' AND model='write';"), b"0")
            finally:
                owner.plan["prefix_manifest_sha384"] = prefix
                extra_path.unlink()

            reference_dump = stage / "canonical-prefix.dump"
            dump(original, reference_dump)
            reference = create("reference")
            owner.restore_archive(reference, reference_dump)
            owner.plan["restored_target_catalog"] = fingerprint(reference)["catalog"]
            owner.isolated_migrate(reference, image_ids[len(history)])
            owner.plan["restored_candidate_catalog"] = fingerprint(reference)["catalog"]
            fresh = create("fresh")
            owner.isolated_migrate(fresh, image_ids[len(history)])
            delta = schema_contract.forward_catalog_delta(canonical["catalog"], fingerprint(fresh)["catalog"],
                prefix_manifest=prefix, installed_prefix=6)
            owner.plan["source_delta"] = delta
            with owner.database_target(original):
                existing = json.loads(owner.sql("SELECT json_agg(json_build_array(app_label,model)) "
                                                "FROM public.inventory_content_type;"))
                pending_keys = [key for key in keys if key not in existing]
                self.assertGreater(len(pending_keys), 1)
                required_id, custom_id = str(uuid.uuid4()), str(uuid.uuid4())
                key = pending_keys[0]
                owner.sql("INSERT INTO public.inventory_content_type(id,app_label,model) VALUES ("
                          + literal(required_id) + "," + literal(key[0]) + "," + literal(key[1]) + "),("
                          + literal(custom_id) + ",'pip171_custom','original');")
                baseline = owner.fingerprint()
                expectation = owner.migration_expectation(baseline)
                deploy.write_json(stage / "migration-expectation.json", expectation)
                owner.plan.update(installed_target_catalog=baseline["catalog"],
                    installed_candidate_additions=owner.target_additions(owner.append_slots()),
                    installed_candidate_replacements=delta["replacements"],
                    installed_candidate_removals=delta["removals"])
            backup = stage / "registry-before.dump"
            dump(original, backup)

            def restored():
                database = create("restore")
                owner.restore_archive(database, backup)
                self.assertEqual(fingerprint(database), {**baseline, "catalog": owner.plan["restored_target_catalog"]})
                return database

            clone = restored()
            with owner.database_target(clone):
                self.assertEqual(owner.migration_expectation(baseline, restored=True), expectation)
            for database, is_restore in ((original, False), (clone, True)):
                owner.isolated_migrate(database, image_ids[len(history)])
                with owner.database_target(database):
                    owner.guard_history(completed=True)
                    owner.verify_conservation(baseline, restored=is_restore)
                completed = fingerprint(database)
                owner.isolated_migrate(database, image_ids[len(history)])
                self.assertEqual(fingerprint(database), completed, "registry replay changed actual UUIDs")

            for field in ("original_ids", "native_keys", "context"):
                changed = copy.deepcopy(expectation)
                if field == "context":
                    changed["context"]["helper_sha256"] = "0" * 64
                elif field == "original_ids":
                    changed["content_type_seeds"][field] = []
                else:
                    changed["content_type_seeds"][field] = []
                    changed["content_type_seeds"]["native_keys_sha256"] = deploy.sha(deploy.canonical([]))
                (stage / "migration-expectation.json").write_bytes(deploy.canonical(changed))
                try:
                    with owner.database_target(clone), self.assertRaises(deploy.DeployError):
                        owner.verify_conservation(baseline, restored=True)
                finally:
                    (stage / "migration-expectation.json").write_bytes(deploy.canonical(expectation))

    def prove_prepared_chain(self, root, installed, images, converter, identities, history, prefix, base_reference):
        cohort = root / ("prefix-" + str(installed))
        stage = cohort / "backups/release-20261027T120000Z"
        stage.mkdir(parents=True, mode=0o700)
        cohort.chmod(0o700)
        project = "pwa82-chain-" + uuid.uuid4().hex
        owner = deploy.Deployment(argparse.Namespace(root=cohort, stage=stage, project_name=project))
        image_ids = {n: json.loads(deploy.run(["docker", "image", "inspect", image]))[0]["Id"]
                     for n, image in images.items()}
        owner.plan = {
            "migrations": {str(row["version"]): row["checksum"] for row in history},
            "contract": json.loads(self.CONTRACT_PATH.read_bytes()),
            "prefix_manifest_sha384": prefix, "commit": identities["source_sha"],
            "platform": "linux/amd64",
            "disabled_schedule": None, "image_file": ".env", "preservation_sha256": None,
            "postgres_image": os.environ.get("PIPING_NATIVE_POSTGRES_IMAGE", "postgres:18-trixie"),
            "images": {"backend": {"local_id": image_ids[len(history)]}},
            "installed_backend_image": image_ids[installed],
        }
        # Stage the candidate's checksum-bound transform source through the same
        # image extraction owner used by a real deployment.
        candidate_migrations, candidate_contract, candidate_contract_sha = owner.published_migrations()
        self.assertEqual(candidate_migrations, owner.plan["migrations"])
        self.assertEqual(candidate_contract, owner.plan["contract"])
        self.assertEqual(candidate_contract_sha, deploy.file_sha(self.CONTRACT_PATH))
        deploy.validate_contract(candidate_contract, candidate_migrations)
        seed_source = stage / "source-migrations" / Path(schema_contract.CONTENT_TYPE_SEED_MIGRATIONS[8][0]).name
        seed_bytes = deploy.read(seed_source)
        seed_source.write_bytes(seed_bytes + b"\n-- substituted\n")
        try:
            with self.assertRaises(deploy.DeployError):
                owner.source_migration(seed_source.name)
        finally:
            seed_source.write_bytes(seed_bytes)
        with owner.isolated_database("synthetic-prefix-" + str(installed)) as cluster:
            def target(name):
                return {**cluster, "name": name, "url": cluster["url"].rsplit("/", 1)[0] + "/" + name}

            def create(label):
                guarded()
                name = "synthetic_" + label + "_" + uuid.uuid4().hex[:12]
                with owner.database_target(cluster):
                    owner.sql("CREATE DATABASE " + name + ";")
                return target(name)

            database = target("synthetic_original_" + uuid.uuid4().hex[:12])
            # This is an exclusively owned fixture configuration, not an installed
            # secret. Its nondefault reference exercises the settings-owned cipher.
            environment = {
                "PIPING_DATABASE__URL": database["url"],
                "PIPING_SECRETS__CIPHER__KEY_MATERIAL_REF": "env:PWA82_SYNTHETIC_CIPHER",
                "PWA82_SYNTHETIC_CIPHER": base64.b64encode(os.urandom(32)).decode(),
                "PIPING_AUTH__PASSWORD_PEPPER": uuid.uuid4().hex,
                "PIPING_AUTH__LOCAL_ENABLED": "false", "PIPING_OBSERVABILITY__OTEL_ENABLED": "false",
                "PIPING_REDIS__URL": "redis://127.0.0.1:1",
            }
            deploy.write(cohort / ".env", b"")
            # Writer sentinels have the same effective environment and paired image,
            # but deliberately do not start the backend/migrations during R setup.
            deploy.write_json(cohort / "docker-compose.yml", {"services": {
                role: {"image": image_ids[installed], "entrypoint": ["tail", "-f", "/dev/null"],
                       "environment": environment, "network_mode": "none"} for role in deploy.WRITERS}})
            try:
                deploy.run(owner.compose() + ["up", "-d", "--pull", "never"])
                config = json.loads(deploy.run(owner.compose() + ["config", "--format", "json"]))
                effective = config["services"]["backend"]["environment"]
                self.assertTrue(effective == environment, "effective backend environment mismatch")
                deploy.write(stage / "proof-backend.env", "".join(k + "=" + v + "\n" for k, v in effective.items()).encode())
                proof_digest = deploy.file_sha(stage / "proof-backend.env")
                owner.plan["baseline"] = {name: deploy.file_sha(cohort / name)
                                          for name in (".env", "docker-compose.yml")}
                owner.plan["writer_ids"] = {
                    role: deploy.run(owner.compose() + ["ps", "-q", role]).decode().strip() for role in deploy.WRITERS}
                self.assertTrue(all(owner.plan["writer_ids"].values()), "writer sentinel missing")
                for identifier in owner.plan["writer_ids"].values():
                    inspected = json.loads(deploy.run(["docker", "inspect", identifier]))[0]
                    self.assertEqual(inspected["Image"], image_ids[installed])
                    actual = dict(entry.split("=", 1) for entry in inspected["Config"]["Env"])
                    self.assertTrue(all(actual.get(k) == v for k, v in effective.items()),
                                    "writer effective cipher configuration mismatch")
                deploy.write_json(stage / "synthetic-identities.json", {
                    **identities, "writer_ids": owner.plan["writer_ids"], "cohort_image_ids": image_ids,
                    "configuration_sha256": owner.plan["baseline"], "proof_environment_sha256": proof_digest})
                deploy.run(owner.compose() + ["stop", "--timeout", "60", *deploy.WRITERS])

                def guarded():
                    owner.stopped()
                    owner.guard_writer_ids()
                    owner.guard_config()
                    deploy.require(deploy.file_sha(stage / "proof-backend.env") == proof_digest,
                                   "effective backend configuration substituted")
                    deploy.require(deploy.file_sha(converter) == identities["converter_sha256"],
                                   "converter artifact substituted")
                    deploy.require(deploy.file_sha(Path(os.environ["PIPING_NATIVE_BACKEND_BINARY"]))
                                   == identities["backend_sha256"], "backend artifact substituted")
                    deploy.require(json.loads(deploy.run(["docker", "image", "inspect", base_reference]))[0]["Id"]
                                   == identities["backend_image_id"], "cohort build base substituted")
                    for n, image in images.items():
                        deploy.require(json.loads(deploy.run(["docker", "image", "inspect", image]))[0]["Id"]
                                       == image_ids[n], "cohort image substituted")

                def migrate(db, image=None):
                    guarded()
                    owner.isolated_migrate(db, image or image_ids[len(history)])

                def convert(db, executable=converter):
                    guarded()
                    deploy.require(executable == converter and deploy.file_sha(executable) == identities["converter_sha256"],
                                   "unverified converter executable")
                    # Reuse the deployment owner's private isolated URL/environment.
                    with owner.isolated_environment(db) as private_environment:
                        deploy.run(["docker", "run", "--rm", "--pull", "never", "--network", db["network"],
                                    "--cpus", "1", "--memory", "1g", "--env-file", str(private_environment),
                                    "--entrypoint", "/usr/local/bin/convert-fhrp-auth-keys", image_ids[4],
                                    "--offline-confirmed", "--backup-confirmed", "--convert-fhrp-auth-keys"], timeout=300)

                def fingerprint(db):
                    with owner.database_target(db):
                        return owner.fingerprint()

                def dump(db, path):
                    guarded()
                    with path.open("xb") as output:
                        path.chmod(0o600)
                        deploy.run(db["compose"] + ["exec", "-T", "postgres", "pg_dump", "-U", "piping",
                                   "-d", db["name"], "-Fc", "--no-owner", "--no-privileges"], output=output, timeout=300)
                    return deploy.file_sha(path)

                def restore(path, digest, baseline, expected_catalog):
                    guarded()
                    deploy.require(deploy.file_sha(path) == digest, "sealed synthetic archive changed")
                    clone = create("restore")
                    owner.restore_archive(clone, path)
                    deploy.require(fingerprint(clone) == {**baseline, "catalog": expected_catalog},
                                   "synthetic restore changed sealed source state")
                    return clone

                guarded()
                with owner.database_target(cluster):
                    owner.sql("CREATE DATABASE " + database["name"] + ";")
                migrate(database, image_ids[installed])
                # Establish catalog compaction independently from a genuine empty
                # prefix, before R exists. Never approve observed R/C drift itself.
                reference = create("reference")
                migrate(reference, image_ids[installed])
                reference_fingerprint = fingerprint(reference)
                native_catalog = reference_fingerprint["catalog"]
                owner.plan["installed_history"] = reference_fingerprint["history"]
                owner.plan["content_type_seed_keys"] = owner.native_content_type_seed_keys(reference)
                with owner.database_target(database):
                    existing_keys = json.loads(owner.sql(
                        "SELECT coalesce(json_agg(json_build_array(app_label,model)),'[]'::json) "
                        "FROM public.inventory_content_type;"))
                    missing_keys = [key for key in owner.plan["content_type_seed_keys"] if key not in existing_keys]
                    self.assertTrue(missing_keys, "source rehearsal requires a genuinely pending registry key")
                    required_id, custom_id = str(uuid.uuid4()), str(uuid.uuid4())
                    key = missing_keys[0]
                    def literal(value):
                        return "'" + value.replace("'", "''") + "'"
                    owner.sql("INSERT INTO public.inventory_content_type(id,app_label,model) VALUES ("
                              + literal(required_id) + "," + literal(key[0]) + "," + literal(key[1]) + "),("
                              + literal(custom_id) + ",'pip171_custom'," + literal("model_" + uuid.uuid4().hex) + ");")
                reference_dump = stage / "reference-prefix.dump"
                dump(reference, reference_dump)
                restored_reference = create("reference_restore")
                owner.restore_archive(restored_reference, reference_dump)
                restored_catalog = fingerprint(restored_reference)["catalog"]
                migrate(restored_reference)
                restored_candidate_catalog = fingerprint(restored_reference)["catalog"]
                fresh = create("fresh")
                migrate(fresh)
                fresh_before = fingerprint(fresh)
                migrate(fresh)
                self.assertTrue(fingerprint(fresh) == fresh_before, "fresh migration replay changed state")
                delta = schema_contract.forward_catalog_delta(native_catalog, fresh_before["catalog"],
                    prefix_manifest=prefix, installed_prefix=installed)
                owner.plan["source_delta"] = delta
                owner.plan["restored_target_catalog"] = restored_catalog
                owner.plan["restored_candidate_catalog"] = restored_candidate_catalog

                # Seed a real preexisting encrypted graph through the canonical
                # converter, with its own stopped-writer backup and restore first.
                old_id, new_id, unicode_id, empty_id = (str(uuid.uuid4()) for _ in range(4))
                pending_ids = (new_id, unicode_id)
                guarded()
                with owner.database_target(database):
                    # The full chain requires legacy memberships to resolve to
                    # pre-existing Tag identities, not allocate unknown tokens.
                    owner.sql("INSERT INTO inventory_tag(name,slug) VALUES "
                               "('old','old'),('keep','keep'),('unicode','unicode'),('tag','tag');"
                               "INSERT INTO ipam_fhrpgroup(id,custom_field_data,group_id,name,protocol,auth_type,auth_key,"
                              "description,comments,tags) VALUES ('" + old_id + "','{}',11,'synthetic old graph','vrrp',"
                              "'plaintext','synthetic preexisting credential','old description','old comment',ARRAY['old']);")
                seed_before = fingerprint(database)
                seed_dump = stage / "synthetic-seed-original.dump"
                seed_digest = dump(database, seed_dump)
                restore(seed_dump, seed_digest, seed_before, restored_catalog)
                convert(database)
                guarded()
                with owner.database_target(database):
                    owner.sql("INSERT INTO ipam_fhrpgroup(id,custom_field_data,group_id,name,protocol,auth_type,auth_key,"
                              "description,comments,tags) VALUES ('" + new_id + "','{\"keep\":true}',12,'synthetic conversion',"
                               "'vrrp','plaintext','  synthetic exact bytes  ','keep description','keep comment',ARRAY['keep']),"
                               "('" + unicode_id + "','{\"unicode\":true}',14,'synthetic Unicode conversion',"
                               "'vrrp','plaintext','  синтетический ключ 認証 🔐  ','Unicode description','Unicode comment',ARRAY['unicode']),"
                              "('" + empty_id + "','{}',13,'synthetic empty','vrrp',NULL,'','empty description','empty comment',ARRAY[]::text[]);"
                              "UPDATE instance_branding SET app_name='Synthetic PWA-82 rehearsal';")
                    self.seed_service(owner, installed)
                original = fingerprint(database)
                self.assertTrue(original["catalog"] == native_catalog, "fixture setup changed genuine prefix catalog")
                owner.plan["installed_history"] = original["history"]
                self.assertEqual(len(original["history"]), installed)
                with owner.database_target(database):
                    owner.guard_history()
                original_dump = stage / "original-r.dump"
                original_digest = dump(database, original_dump)
                deploy.write_json(stage / "original-r-fingerprint.json", original)
                original_restored = restore(original_dump, original_digest, original, restored_catalog)
                old_graph = self.graph(owner, database)
                self.assertTrue(any(row["consumer_id"] == old_id and row["field_key"] == "auth_key"
                                    and row["owner_scope_id"] == old_id
                                    and row["consumer_type"] == row["owner_scope_type"] == "inventory_fhrp_group"
                                    for row in old_graph["secret_usage_binding"]),
                                "canonical seed did not create the preexisting encrypted graph")
                original_fields = self.non_key_groups(owner, database)
                self.prove_conversion_failures(owner, restore, dump, convert, guarded, fingerprint,
                    original_dump, original_digest, original, restored_catalog, pending_ids)
                convert(database)
                prepared = fingerprint(database)
                semantics = self.assert_conversion(owner, database, original, old_graph, original_fields, pending_ids, empty_id)
                prepared_graph = self.graph(owner, database)
                # R replay uses fresh UUIDs/nonces: compare semantic identities, not
                # newly generated ciphertext. All old graph rows stay byte-exact.
                convert(original_restored)
                replay_semantics = self.assert_conversion(owner, original_restored, original, old_graph,
                                                         original_fields, pending_ids, empty_id)
                self.assertTrue(semantics == replay_semantics, "R replay changed conversion semantics")
                convert(database)
                self.assertTrue(fingerprint(database) == prepared, "prepared conversion replay changed C")
                prepared_dump = stage / "offline-pre-upgrade.dump"
                prepared_digest = dump(database, prepared_dump)
                self.assertNotEqual(original_digest, prepared_digest)
                deploy.write_json(stage / "fingerprint.json", prepared)
                restored = restore(prepared_dump, prepared_digest, prepared, restored_catalog)
                with owner.database_target(database):
                    expectation = owner.migration_expectation(prepared)
                    deploy.write_json(stage / "migration-expectation.json", expectation)
                    slots = owner.append_slots()
                    owner.plan.update(installed_target_catalog=prepared["catalog"],
                                      installed_candidate_additions=owner.target_additions(slots),
                                      installed_candidate_replacements=delta["replacements"],
                                      installed_candidate_removals=delta["removals"])
                with owner.database_target(restored):
                    self.assertTrue(owner.migration_expectation(prepared, restored=True) == expectation,
                                    "C restore did not reproduce its expectation")
                for db, is_restore in ((database, False), (restored, True)):
                    migrate(db)
                    with owner.database_target(db):
                        owner.guard_history(completed=True)
                        owner.verify_conservation(prepared, restored=is_restore)
                        self.assertEqual(owner.sql("SELECT count(*) FROM public.inventory_content_type WHERE id IN ('"
                                                   + required_id + "','" + custom_id + "');"), b"2")
                        if installed < 3:
                            self.assertEqual(json.loads(owner.sql(
                                "SELECT to_json(port_mappings) FROM ipam_service WHERE name='mixed';")),
                                ["udp/53", "tcp/80", "sctp/7"])
                    self.assertTrue(self.graph(owner, db) == prepared_graph, "DDL changed sealed C graph/ciphertext")
                    after = fingerprint(db)
                    migrate(db)
                    self.assertTrue(fingerprint(db) == after, "DDL replay changed state")
                    with self.assertRaises(deploy.DeployError):
                        convert(db)
                    self.assertTrue(fingerprint(db) == after, "conversion after DDL changed state")
                self.prove_chain_negatives(owner, create, restore, migrate, convert, guarded, fingerprint,
                    original_dump, original_digest, original, prepared_dump, prepared_digest, prepared,
                    restored_catalog, expectation, converter, installed, root, database, required_id, custom_id)
                guarded()
                self.assertFalse((root / "fhrp-conversion-chain-receipt.json").exists())
                return {"installed_prefix": installed, "original_r_dump_sha256": original_digest,
                        "prepared_c_dump_sha256": prepared_digest}
            finally:
                # Cleanup removes only this cohort; writers are never restarted.
                deploy.run(owner.compose() + ["down", "--volumes", "--timeout", "30"], timeout=90)

    def graph(self, owner, database):
        with owner.database_target(database):
            return {table: json.loads(owner.sql("SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),"
                                               "'[]'::jsonb) FROM public." + table + " t;")) for table in self.GRAPH}

    def non_key_groups(self, owner, database):
        with owner.database_target(database):
            return owner.sql("SELECT coalesce(jsonb_agg(to_jsonb(g)-'auth_key' ORDER BY id),'[]'::jsonb) FROM ipam_fhrpgroup g;")

    def prove_conversion_failures(self, owner, restore, dump, convert, guarded, fingerprint,
                                  original_dump, original_digest, original, restored_catalog, pending_ids):
        for failure in ("invalid_legacy", "late_clear"):
            clone = restore(original_dump, original_digest, original, restored_catalog)
            guarded()
            marker = "pwa82_conversion_rollback_" + uuid.uuid4().hex
            with owner.database_target(clone):
                if failure == "invalid_legacy":
                    owner.sql("UPDATE ipam_fhrpgroup SET auth_key=E' \\t ' WHERE id='" + pending_ids[-1] + "';")
                else:
                    group_ids = ",".join("'" + group_id + "'::uuid" for group_id in pending_ids)
                    # AFTER STATEMENT runs after canonical lifecycle writes and
                    # source clearing. The private server-log marker is outside
                    # transaction rollback; no row/sequence marker may weaken
                    # the complete-state equality check below.
                    owner.sql("""
CREATE FUNCTION public.pwa82_refuse_conversion_clear() RETURNS trigger
LANGUAGE plpgsql AS $pwa82_late$
BEGIN
 IF (SELECT count(DISTINCT u.consumer_id)
     FROM secret_usage_binding u
     JOIN secrets_variable v ON v.id=u.secret_id
     JOIN secrets_store s ON s.secret_id=u.secret_id
     JOIN secrets_binding b ON b.secret_id=u.secret_id
       AND b.scope_type=u.owner_scope_type AND b.scope_id=u.owner_scope_id
     WHERE u.consumer_id IN (""" + group_ids + """)
       AND u.consumer_type='inventory_fhrp_group' AND u.field_key='auth_key'
       AND u.owner_scope_type='inventory_fhrp_group' AND u.owner_scope_id=u.consumer_id
       AND v.kind='field' AND v.retire_with_field_usage
       AND b.managed_by_field_usage AND s.encrypted_value<>'')=""" + str(len(pending_ids)) + """
    AND NOT EXISTS (SELECT FROM ipam_fhrpgroup
                    WHERE id IN (""" + group_ids + """) AND auth_key<>'')
 THEN
   RAISE LOG '""" + marker + """';
   RAISE EXCEPTION 'synthetic conversion failure after lifecycle writes';
 END IF;
 RAISE EXCEPTION 'synthetic late-failure precondition not reached';
END; $pwa82_late$;
CREATE TRIGGER pwa82_refuse_conversion_clear AFTER UPDATE ON public.ipam_fhrpgroup
FOR EACH STATEMENT EXECUTE FUNCTION public.pwa82_refuse_conversion_clear();
""")
            # Seal the exact negative state, including its trigger/catalog, and
            # independently restore its own backup before attesting conversion.
            before = fingerprint(clone)
            before_graph = self.graph(owner, clone)
            deploy.write_json(owner.stage / ("conversion-" + failure + "-before.json"), before)
            archive = owner.stage / ("conversion-" + failure + "-r.dump")
            digest = dump(clone, archive)
            restore(archive, digest, before, before["catalog"])
            with self.assertRaises(deploy.DeployError):
                convert(clone)
            self.assertTrue(fingerprint(clone) == before,
                            "refused conversion changed R, catalog, rows, sequences or ledger")
            self.assertTrue(self.graph(owner, clone) == before_graph,
                            "refused conversion changed preexisting or pending secret graph")
            if failure == "late_clear":
                private_logs = deploy.run(clone["compose"] + ["logs", "--no-color", "postgres"])
                self.assertTrue(marker.encode() in private_logs,
                                "late refusal did not reach the post-lifecycle native trigger")
            guarded()

    def assert_conversion(self, owner, database, original, old_graph, original_fields, pending_ids, empty_id):
        graph = self.graph(owner, database)
        self.assertTrue(self.non_key_groups(owner, database) == original_fields, "conversion changed non-key FHRP fields")
        for table in self.GRAPH:
            self.assertTrue(all(row in graph[table] for row in old_graph[table]), "conversion changed preexisting secret graph")
            self.assertEqual(len(graph[table]), len(old_graph[table]) + len(pending_ids), "unexpected conversion graph footprint")
        added = {table: [row for row in graph[table] if row not in old_graph[table]] for table in self.GRAPH}
        with owner.database_target(database):
            converted = owner.fingerprint()
            untouched = set(original["rows"]) - {"public.ipam_fhrpgroup", *("public." + t for t in self.GRAPH)}
            self.assertTrue(all(converted["rows"][t] == original["rows"][t] for t in untouched),
                            "conversion changed unrelated values, sequences or ledger")
            self.assertTrue(converted["history"] == original["history"], "conversion changed ledger metadata")
            self.assertEqual(owner.sql("SELECT NOT EXISTS (SELECT FROM ipam_fhrpgroup WHERE auth_key <> '');"), b"t")
            semantics, secret_ids = [], set()
            for group_id in sorted(pending_ids):
                usage = [row for row in added["secret_usage_binding"] if row["consumer_id"] == group_id]
                self.assertEqual(len(usage), 1, "converted group must have exactly one new usage")
                row = usage[0]
                self.assertTrue(row["consumer_type"] == row["owner_scope_type"] == "inventory_fhrp_group"
                                and row["consumer_id"] == row["owner_scope_id"] == group_id and row["field_key"] == "auth_key",
                                "conversion usage identity mismatch")
                secret_id = row["secret_id"]
                self.assertTrue(secret_id not in secret_ids, "converted groups unexpectedly share a secret")
                secret_ids.add(secret_id)
                variables = [v for v in added["secrets_variable"] if v["id"] == secret_id]
                bindings = [b for b in added["secrets_binding"] if b["secret_id"] == secret_id]
                stores = [s for s in added["secrets_store"] if s["secret_id"] == secret_id]
                self.assertEqual((len(variables), len(bindings), len(stores)), (1, 1, 1),
                                 "converted group graph correspondence is incomplete or ambiguous")
                variable, binding, store = variables[0], bindings[0], stores[0]
                self.assertTrue(variable["kind"] == "field" and variable["retire_with_field_usage"]
                                and variable["key"].startswith("secret-field/"), "conversion variable identity mismatch")
                self.assertTrue(binding["scope_type"] == "inventory_fhrp_group" and binding["scope_id"] == group_id
                                and binding["managed_by_field_usage"] and binding["weight"] == 0,
                                "conversion owner binding mismatch")
                self.assertTrue(bool(store["encrypted_value"]), "conversion encrypted store row missing")
                semantics.append({k: row[k] for k in (
                    "consumer_type", "consumer_id", "owner_scope_type", "owner_scope_id", "field_key")})
            self.assertTrue({v["id"] for v in added["secrets_variable"]} == secret_ids
                            == {s["secret_id"] for s in added["secrets_store"]}
                            == {b["secret_id"] for b in added["secrets_binding"]},
                            "conversion created unrelated or missing secret graph rows")
            self.assertFalse(any(u["consumer_id"] == empty_id for u in graph["secret_usage_binding"]),
                             "empty group acquired a secret")
            return semantics

    def seed_service(self, owner, installed):
        if installed < 3:
            owner.sql("""
INSERT INTO ipam_service(id,name,protocol,ports,description,comments,custom_field_data,parent_object_id,parent_object_type_id,tags)
 VALUES ('01900000-0000-7000-8000-000000000093','ordered','tcp',ARRAY[443,80],'description','comment','{"keep":true}',uuidv7(),uuidv7(),ARRAY['tag']),
 ('01900000-0000-7000-8000-000000000095','mixed',NULL,NULL,'','', '{}',uuidv7(),uuidv7(),ARRAY[]::text[]);
INSERT INTO ipam_service_port_mapping(service_id,protocol,port,ordinal) VALUES
 ('01900000-0000-7000-8000-000000000093','tcp',443,8),('01900000-0000-7000-8000-000000000093','tcp',80,2),
 ('01900000-0000-7000-8000-000000000095','tcp',80,2),('01900000-0000-7000-8000-000000000095','udp',53,1),
 ('01900000-0000-7000-8000-000000000095','sctp',7,3);
INSERT INTO ipam_servicetemplate(id,name,protocol,ports,description,comments,custom_field_data)
 VALUES ('01900000-0000-7000-8000-000000000094','template','sctp',ARRAY[65535,1],'description','comment','{"keep":true}');
""")
        else:
            owner.sql("""
INSERT INTO ipam_service(id,name,port_mappings,description,comments,custom_field_data,parent_object_id,parent_object_type_id,tags)
 VALUES ('01900000-0000-7000-8000-000000000093','ordered',ARRAY['tcp/80','tcp/443'],'description','comment','{"keep":true}',uuidv7(),uuidv7(),ARRAY['tag']);
""")
        owner.sql("""
INSERT INTO ipam_ipaddress(id,address,status,dns_name,description,comments,custom_field_data)
 VALUES ('01900000-0000-7000-8000-000000000094','192.0.2.93/32','active','','','','{}');
INSERT INTO ipam_service_ipaddresses(service_id,ipaddress_id)
 VALUES ('01900000-0000-7000-8000-000000000093','01900000-0000-7000-8000-000000000094');
""")

    def prove_chain_negatives(self, owner, create, restore, migrate, convert, guarded, fingerprint,
                              r_dump, r_digest, original, c_dump, c_digest, prepared, restored_catalog,
                              expectation, converter, installed, root, completed, required_id, custom_id):
        for path, digest, baseline in ((r_dump, r_digest, original), (c_dump, c_digest, prepared)):
            for corruption in ("truncated", "tampered"):
                bad = owner.stage / (path.stem + "-" + corruption + ".dump")
                raw = path.read_bytes()
                bad.write_bytes(raw[:32] if corruption == "truncated" else raw[:-1] + bytes([raw[-1] ^ 1]))
                bad.chmod(0o600)
                with self.assertRaises(deploy.DeployError):
                    restore(bad, digest, baseline, restored_catalog)
                if corruption == "truncated":
                    with self.assertRaises(deploy.DeployError):
                        owner.restore_archive(create("truncated"), bad)
        # These clones retain row counts. Exact graph/ciphertext conservation, not
        # converter count output, must detect every mutation.
        mutations = {
            "ciphertext": "UPDATE secrets_store SET encrypted_value=encrypted_value || 'tampered';",
            "binding": "UPDATE secrets_binding SET weight=weight+1;",
            "usage": "UPDATE secret_usage_binding SET field_key='tampered';",
            "attribute": "UPDATE ipam_service SET comments='tampered';",
            "edge": "UPDATE ipam_service_ipaddresses SET ipaddress_id=uuidv7();",
        }
        for label, mutation in mutations.items():
            clone = restore(c_dump, c_digest, prepared, restored_catalog)
            if label == "edge":
                # Use a valid alternate target so this exercises conservation,
                # rather than relying on a foreign-key rejection.
                mutation = ("INSERT INTO ipam_ipaddress(id,address,status,dns_name,description,comments,custom_field_data) "
                            "VALUES ('01900000-0000-7000-8000-000000000099','192.0.2.99/32','active','','','','{}');"
                            "UPDATE ipam_service_ipaddresses SET ipaddress_id='01900000-0000-7000-8000-000000000099';")
            with owner.database_target(clone):
                owner.sql(mutation)
                with self.assertRaises(deploy.DeployError):
                    owner.migration_expectation(prepared, restored=True)
            guarded()
        plaintext = restore(r_dump, r_digest, original, restored_catalog)
        with owner.database_target(plaintext):
            with self.assertRaises(deploy.DeployError):
                owner.migration_expectation(original, restored=True)
        with self.assertRaises(deploy.DeployError):
            migrate(plaintext)
        # SQLx commits per migration: at prefixes 1/2 an earlier authentic tail may
        # have appended before 0004 refuses. The plaintext column must survive.
        with owner.database_target(plaintext):
            self.assertEqual(owner.sql("SELECT EXISTS (SELECT FROM ipam_fhrpgroup WHERE auth_key <> '');"), b"t")
        substituted = owner.stage / "unverified-converter"
        substituted.write_bytes(b"not the paired converter")
        clone = restore(r_dump, r_digest, original, restored_catalog)
        before = fingerprint(clone)
        with self.assertRaises(deploy.DeployError):
            convert(clone, substituted)
        self.assertTrue(fingerprint(clone) == before, "substituted converter mutated R")
        proof = owner.stage / "proof-backend.env"
        sealed_environment = proof.read_bytes()
        substituted_environment = deploy.read_proof_environment(proof)
        substituted_environment["PWA82_SYNTHETIC_CIPHER"] = base64.b64encode(os.urandom(32)).decode()
        proof.write_bytes("".join(k + "=" + v + "\n" for k, v in substituted_environment.items()).encode())
        try:
            with self.assertRaises(deploy.DeployError):
                convert(clone)
        finally:
            proof.write_bytes(sealed_environment)
        self.assertTrue(fingerprint(clone) == before, "substituted configuration mutated R")
        writer = deploy.WRITERS[0]
        writer_identity = owner.plan["writer_ids"][writer]
        owner.plan["writer_ids"][writer] = "f" * 64
        try:
            with self.assertRaises(deploy.DeployError):
                convert(clone)
        finally:
            owner.plan["writer_ids"][writer] = writer_identity
        self.assertTrue(fingerprint(clone) == before, "substituted writer identity mutated R")
        expectation_path = owner.stage / "migration-expectation.json"
        for key in ("application_source", "raw_fingerprint_sha256", "transforms"):
            changed = copy.deepcopy(expectation)
            changed["context"][key] = [] if key == "transforms" else "f" * 64
            expectation_path.write_bytes(deploy.canonical(changed))
            try:
                with owner.database_target(completed), self.assertRaises(deploy.DeployError):
                    owner.verify_conservation(prepared)
            finally:
                expectation_path.write_bytes(deploy.canonical(expectation))
        content_types = expectation.get("content_type_seeds")
        if content_types is not None:
            self.assertTrue(content_types["original_ids"] and content_types["native_keys"])
            for field in ("original_ids", "native_keys"):
                changed = copy.deepcopy(expectation)
                typed = changed["content_type_seeds"]
                if field == "original_ids":
                    typed[field] = sorted([str(uuid.uuid4()), *typed[field][1:]])
                else:
                    typed[field] = []
                    typed["native_keys_sha256"] = deploy.sha(deploy.canonical(typed[field]))
                expectation_path.write_bytes(deploy.canonical(changed))
                try:
                    with owner.database_target(completed), self.assertRaises(deploy.DeployError):
                        owner.verify_conservation(prepared)
                finally:
                    expectation_path.write_bytes(deploy.canonical(expectation))
            registry_cases = ("old-deleted", "old-id-replaced", "old-key-changed", "missing-seed", "extra-key")
            for case in registry_cases:
                registry_clone = restore(c_dump, c_digest, prepared, restored_catalog)
                migrate(registry_clone)
                with owner.database_target(registry_clone):
                    if case == "old-deleted":
                        statement = "DELETE FROM public.inventory_content_type WHERE id='" + custom_id + "';"
                    elif case == "old-id-replaced":
                        statement = ("UPDATE public.inventory_content_type SET id='" + str(uuid.uuid4())
                                     + "' WHERE id='" + custom_id + "';")
                    elif case == "old-key-changed":
                        statement = ("UPDATE public.inventory_content_type SET model='changed_" + uuid.uuid4().hex
                                     + "' WHERE id='" + custom_id + "';")
                    elif case == "extra-key":
                        statement = ("INSERT INTO public.inventory_content_type(id,app_label,model) VALUES ('"
                                     + str(uuid.uuid4()) + "','pip171_extra','model_" + uuid.uuid4().hex + "');")
                    else:
                        rows = json.loads(owner.sql("SELECT coalesce(json_agg(json_build_array(id::text,app_label,model)),"
                                                    "'[]'::json) FROM public.inventory_content_type;"))
                        new_ids = [row[0] for row in rows if row[1:] in content_types["native_keys"]
                                   and row[0] not in content_types["original_ids"]]
                        self.assertTrue(new_ids, "native source must create a pending registry key")
                        statement = "DELETE FROM public.inventory_content_type WHERE id='" + new_ids[0] + "';"
                    owner.sql(statement)
                    with self.assertRaises(deploy.DeployError):
                        owner.verify_conservation(prepared, restored=True)
                guarded()
        # Precise native ledger refusals use clones, never forged accepted cohorts.
        cases = {
            "unknown": "UPDATE _sqlx_migrations SET version=99 WHERE version=4;",
            "failed": "UPDATE _sqlx_migrations SET success=false WHERE version=4;",
            "checksum": "UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=2;",
            "identity": "UPDATE _sqlx_migrations SET description='foreign' WHERE version=2;",
            "gap": "DELETE FROM _sqlx_migrations WHERE version=2;",
            "reordered": "UPDATE _sqlx_migrations SET installed_on=installed_on + interval '1 day' WHERE version=1;",
        }
        # A C dump is restored then authentically migrated for each negative clone.
        for label, mutation in cases.items():
            clone = restore(c_dump, c_digest, prepared, restored_catalog)
            migrate(clone)
            with owner.database_target(clone):
                owner.sql(mutation)
                rejected = owner.fingerprint()
                with self.assertRaises(deploy.DeployError):
                    owner.guard_history(completed=True)
            if label in ("unknown", "failed", "checksum"):
                with self.assertRaises(deploy.DeployError):
                    migrate(clone)
                self.assertTrue(fingerprint(clone) == rejected, "rejected ledger migration changed state")
        for label, ddl in (("removed", "ALTER TABLE identity_user DROP COLUMN email;"),
                           ("changed", "ALTER TABLE instance_branding ALTER COLUMN app_name TYPE varchar(1000);")):
            clone = restore(c_dump, c_digest, prepared, restored_catalog)
            migrate(clone)
            with owner.database_target(clone):
                before_catalog = owner.catalog()
                owner.sql(ddl)
                with self.assertRaises(ValueError):
                    schema_contract.forward_catalog_delta(before_catalog, owner.catalog(),
                        prefix_manifest=owner.plan["prefix_manifest_sha384"], installed_prefix=installed)
        if installed < 3:
            invalid = restore(c_dump, c_digest, prepared, restored_catalog)
            with owner.database_target(invalid):
                owner.sql("UPDATE ipam_service SET protocol='unknown' WHERE name='ordered';")
                invalid_before = owner.fingerprint()
                with self.assertRaises(deploy.DeployError):
                    owner.migration_expectation(invalid_before, restored=True)
                self.assertTrue(owner.fingerprint() == invalid_before, "invalid Service validation mutated source")
        # Negative-only tail variants never masquerade as product history. They
        # run at genuine prefix3 so a failing 0004 has no earlier committed tail.
        if installed == 3:
            for label, sql in (("rollback", "CREATE TABLE must_rollback(value integer); SELECT 1/0;"),
                               ("unauthorized_drop", "ALTER TABLE identity_user DROP COLUMN email;")):
                context = root / ("negative-tail-" + label)
                shutil.copytree(root / "image-prefix-4", context)
                tail = context / "migrations" / "0004_fhrp_auth_key_secret.sql"
                tail.write_text(sql + "\n")
                tag = "pwa82-negative-" + uuid.uuid4().hex + ":" + label
                guarded()
                deploy.run(["docker", "build", "--pull=false", "--network=none", "-t", tag, str(context)], timeout=300)
                try:
                    clone = restore(c_dump, c_digest, prepared, restored_catalog)
                    before = fingerprint(clone)
                    # This negative image is never accepted as an installation image.
                    with owner.isolated_environment(clone, start_mode="migrate") as env_file:
                        command = ["docker", "run", "--rm", "--pull", "never", "--network", clone["network"],
                                   "--env-file", str(env_file), tag]
                        guarded()
                        if label == "rollback":
                            with self.assertRaises(deploy.DeployError):
                                deploy.run(command, timeout=300)
                            self.assertTrue(fingerprint(clone) == before, "failed SQL tail did not roll back")
                        else:
                            deploy.run(command, timeout=300)
                            with self.assertRaises(ValueError):
                                schema_contract.forward_catalog_delta(before["catalog"], fingerprint(clone)["catalog"],
                                    prefix_manifest=owner.plan["prefix_manifest_sha384"], installed_prefix=installed)
                finally:
                    deploy.run(["docker", "image", "rm", tag])
        guarded()



class NativeComposeFixture(unittest.TestCase):
    release: dict[str, Any]
    application_commit = os.environ.get("PIPING_NATIVE_SOURCE_SHA", COMMIT)
    postgres_reference = "postgres:18-alpine"

    @classmethod
    def command(
        cls,
        *args: str,
        input: str | None = None,
        check: bool = True,
        timeout: int = 180,
    ):
        try:
            result = subprocess.run(
                args,
                input=input,
                capture_output=True,
                text=True,
                timeout=timeout,
                check=False,
            )
        except subprocess.SubprocessError:
            # TimeoutExpired otherwise prints complete argv, including the
            # disposable private database/cipher environment of native fixtures.
            raise AssertionError("native fixture command interrupted; private argv suppressed") from None
        if check and result.returncode:
            if hasattr(cls, "root") and cls.root.is_dir():
                private_log = cls.root / ("fixture-command-" + uuid.uuid4().hex + ".private.log")
                private_log.write_text(result.stdout + result.stderr)
                private_log.chmod(0o600)
            # Do not expose env files, keys or captured database/dump output.
            error = AssertionError("native fixture command failed; private output suppressed")
            if re.fullmatch(r"[0-9a-f]{40}", os.environ.get("PIPING_NATIVE_SOURCE_SHA", "")):
                error.native_failure_context = deploy.command_failure_context(
                    args, result.returncode, stderr=result.stderr.encode("utf-8", errors="replace"))
            raise error
        return result

    @classmethod
    def compose(cls, *args: str, **kwargs):
        env_files = [str(cls.root / ".env")]
        if cls.image_file == ".images.env":
            env_files.append(str(cls.root / cls.image_file))
        return cls.command(
            "docker",
            "compose",
            "--project-name",
            cls.project,
            "--project-directory",
            str(cls.root),
            *(part for file in env_files for part in ("--env-file", file)),
            "-f",
            str(cls.root / "docker-compose.yml"),
            "--profile",
            "edge",
            *args,
            **kwargs,
        )

    @classmethod
    def setUpClass(cls):
        cls.initialize_fixture(cls.addClassCleanup)

    @classmethod
    def initialize_fixture(cls, register_cleanup):
        cls.command("docker", "version")
        cls.command("docker", "buildx", "version")
        cls.command("docker", "compose", "version")
        cls.project = "pwa118-native-" + uuid.uuid4().hex[:12]
        cls.image_file = ".images.env"
        artifact_root = Path(
            os.environ.get(
                "PIPING_NATIVE_ARTIFACT_ROOT",
                "/data/artifacts/PWA-118-operator-carried-proof-20261014",
            )
        )
        artifact_root.mkdir(parents=True, exist_ok=True)
        cls.root = Path(tempfile.mkdtemp(prefix=cls.project + "-", dir=artifact_root))
        register_cleanup(cls.cleanup)
        cls.tags = []
        cls.stage_sequence = 0
        cls.key = cls.root / "artifact-signing.pem"
        cls.public_key = cls.root / "artifact-verification.pem"
        cls.command("openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", str(cls.key))
        cls.key.chmod(0o600)
        cls.command("openssl", "pkey", "-in", str(cls.key), "-pubout", "-out", str(cls.public_key))
        cls.public_key.chmod(0o600)
        context = cls.root / "context"
        context.mkdir()
        shutil.copytree(ROOT / "apps/api/migrations", context / "migrations")
        shutil.copytree(ROOT / "ansible/playbooks", context / "ansible/playbooks")
        native_binary = Path(os.environ["PIPING_NATIVE_BACKEND_BINARY"])
        if deploy.file_sha(native_binary) != os.environ["PIPING_NATIVE_BACKEND_SHA256"]:
            raise AssertionError("native fixture executable identity mismatch")
        shutil.copy2(native_binary, context / "piping-backend")
        # Current-baseline transport is a verified pair. Historical fixtures have
        # their own artifact contract and do not claim an FHRP conversion chain.
        converter_copy = ""
        if os.environ.get("PIPING_NATIVE_SOURCE_SHA"):
            converter = Path(os.environ["PIPING_NATIVE_CONVERTER_BINARY"])
            if deploy.file_sha(converter) != os.environ["PIPING_NATIVE_CONVERTER_SHA256"]:
                raise AssertionError("native fixture converter identity mismatch")
            shutil.copy2(converter, context / "convert-fhrp-auth-keys")
            converter_copy = "COPY convert-fhrp-auth-keys /usr/local/bin/convert-fhrp-auth-keys\n"
        (context / "config").mkdir()
        shutil.copyfile(ROOT / "apps/api/config/default.toml", context / "config/default.toml")
        (context / "entrypoint.sh").write_text(
            '#!/bin/sh\nexec /usr/local/bin/piping-backend\n'
        )
        (context / "Dockerfile").write_text(
            "FROM debian:trixie-slim\nARG REVISION\n"
            "RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl libssl3t64 "
            "&& rm -rf /var/lib/apt/lists/*\n"
            "LABEL org.opencontainers.image.revision=$REVISION\n"
            "COPY migrations /app/migrations\nCOPY entrypoint.sh /entrypoint.sh\n"
            "COPY piping-backend /usr/local/bin/piping-backend\nCOPY config /app/config\n"
            "COPY ansible/playbooks /app/ansible/playbooks\nWORKDIR /app\n"
            + converter_copy
            + 'RUN chmod 755 /entrypoint.sh\nENTRYPOINT ["/entrypoint.sh"]\n'
        )
        cls.fixture_image = cls.project + ":fixture"
        cls.tags.append(cls.fixture_image)
        cls.command(
            "docker",
            "build",
            "--build-arg",
            f"REVISION={cls.application_commit}",
            "-t",
            cls.fixture_image,
            str(context),
            timeout=300,
        )
        prefix_context = cls.root / "prefix-context"
        shutil.copytree(context, prefix_context)
        (prefix_context / "migrations/compose-deployment.json").unlink()
        cls.prefix_image = cls.project + ":prefix"
        cls.tags.append(cls.prefix_image)
        cls.command("docker", "build", "--build-arg", f"REVISION={cls.application_commit}", "-t", cls.prefix_image,
                    str(prefix_context), timeout=300)
        (cls.root / ".env").write_text("POSTGRES_USER=piping\nPOSTGRES_DB=piping\nKEEP_ENV=preserve\n")
        (cls.root / ".images.env").write_text(
            "# Keep unrelated settings and this comment.\nKEEP_IMAGE_SETTING=preserve\n"
            + "".join(f"PIPING_{key.upper().replace('-', '_')}_IMAGE={cls.prefix_image}\n" for key in KEYS)
        )
        cls.start_dependency = cls.root / "writer-start-dependency"
        cls.start_dependency.write_bytes(b"owned disposable writer mount")
        cls.start_dependency.chmod(0o600)
        postgres_password = uuid.uuid4().hex
        cls.compose_bytes = json.dumps(
            {
                "services": {
                    "registry": {
                        "image": "registry:2",
                        "cpus": 0.5, "mem_limit": "128m",
                        "ports": ["127.0.0.1::5000"],
                        "healthcheck": {
                            "test": [
                                "CMD",
                                "wget",
                                "-q",
                                "-O",
                                "/dev/null",
                                "http://127.0.0.1:5000/v2/",
                            ],
                            "interval": "1s",
                            "timeout": "3s",
                            "retries": 60,
                        },
                    },
                    "postgres": {
                        "image": cls.postgres_reference,
                        "cpus": 1, "mem_limit": "768m",
                        "environment": {
                            "POSTGRES_USER": "piping",
                            "POSTGRES_DB": "piping",
                            "POSTGRES_PASSWORD": postgres_password,
                        },
                        "healthcheck": {
                            "test": [
                                "CMD",
                                "pg_isready",
                                "-U",
                                "piping",
                                "-d",
                                "piping",
                            ],
                            "interval": "1s",
                            "timeout": "3s",
                            "retries": 60,
                        },
                    },
                    "redis": {
                        "image": "redis:8.10.1", "cpus": 0.5, "mem_limit": "128m",
                        "healthcheck": {"test": ["CMD", "redis-cli", "ping"], "interval": "1s",
                                        "timeout": "3s", "retries": 60},
                    },
                    "backend": {
                        "image": "${PIPING_BACKEND_IMAGE}",
                        "cpus": 1, "mem_limit": "1g",
                        "build": {"context": "/not-an-owned-build-context"},
                        "environment": {"PIPING_DATABASE__URL": f"postgresql://piping:{postgres_password}@postgres:5432/piping",
                                        "PIPING_REDIS__URL": "redis://redis:6379/0",
                                        "PIPING_SECRETS__CIPHER_KEY_B64": SYNTHETIC_CIPHER_KEY,
                                        "PIPING_AUTH__PASSWORD_PEPPER": uuid.uuid4().hex,
                                        "PIPING_AUTH__LOCAL_ENABLED": "false",
                                        "PIPING_SERVER__BIND": "0.0.0.0:7000",
                                        "PIPING_SECURITY__ALLOW_WEAK_DEV_SECRETS": "true",
                                        "PIPING_START_EMBEDDED_GUACD": "false",
                                        "PIPING_OBSERVABILITY__OTEL_ENABLED": "false"},
                        "healthcheck": {"test": ["CMD", "curl", "--fail", "--silent", "--max-time", "10",
                                                  "http://127.0.0.1:7000/api/v1/ready"],
                                        "interval": "1s", "timeout": "12s", "retries": 60},
                    },
                    "ansible-runner": {"image": "${PIPING_ANSIBLE_RUNNER_IMAGE}", "cpus": 0.5, "mem_limit": "128m",
                        "entrypoint": ["/bin/sh", "-c", "exec tail -f /dev/null"],
                        "volumes": [{"type": "bind", "source": str(cls.start_dependency),
                                     "target": "/writer-start-dependency", "read_only": True,
                                     "bind": {"create_host_path": False}}]},
                    "frontend": {"image": "${PIPING_FRONTEND_IMAGE}", "cpus": 0.5, "mem_limit": "128m",
                                 "entrypoint": ["/bin/sh", "-c", "exec tail -f /dev/null"]},
                    "proxy": {"image": "${PIPING_BACKEND_IMAGE}", "profiles": ["edge"], "cpus": 0.5, "mem_limit": "128m",
                              "entrypoint": ["/bin/sh", "-c", "exec tail -f /dev/null"]},
                }
            },
            indent=2,
        ).encode()
        (cls.root / "docker-compose.yml").write_bytes(cls.compose_bytes)
        cls.command("docker", "pull", "redis:8.10.1", timeout=300)
        cls.compose("up", "-d", "--no-build", "--pull", "never", "--wait", "--wait-timeout", "180",
                    "registry")
        # Bootstrap may fetch this fixture's declared image; offline stages may not.
        cls.compose("up", "-d", "--no-build", "--pull", "missing", "--wait", "--wait-timeout", "180",
                    "postgres")
        cls.compose("up", "-d", "--no-build", "--pull", "never", "--wait", "--wait-timeout", "180",
                    "redis")
        cls.registry = "localhost:" + cls.compose("port", "registry", "5000").stdout.strip().rsplit(":", 1)[1]
        # Docker reallocates an anonymous published port even on stop/start.
        # Reserve the selected local endpoint before publishing fixture images.
        topology = json.loads(cls.compose_bytes)
        topology["services"]["registry"]["ports"] = ["127.0.0.1:" + cls.registry.rsplit(":", 1)[1] + ":5000"]
        cls.compose_bytes = json.dumps(topology, indent=2).encode()
        (cls.root / "docker-compose.yml").write_bytes(cls.compose_bytes)
        cls.compose("up", "-d", "--wait", "--wait-timeout", "60", "--no-deps", "registry")
        cls.repositories = {key: f"{cls.registry}/{cls.project}/{key}" for key in KEYS}
        cls.release = {
            "schema": "ci-components.release/v1",
            "release_id": "release-1-" + cls.application_commit[:12],
            "version": "1.2.3",
            "commit": cls.application_commit,
            "pipeline_id": "1",
            "files": [],
            "images": [],
        }
        for key, repository in cls.repositories.items():
            tag = repository + ":fixture"
            cls.tags.append(tag)
            # Match the helper's single-platform release contract; the OCI
            # config digest still differs from the published manifest digest.
            cls.command(
                "docker",
                "buildx",
                "build",
                "--platform",
                "linux/amd64",
                "--provenance=false",
                "--build-arg",
                f"REVISION={cls.application_commit}",
                "--push",
                "-t",
                tag,
                str(context),
                timeout=300,
            )
            raw = cls.command("docker", "buildx", "imagetools", "inspect", "--raw", tag).stdout
            digest = "sha256:" + hashlib.sha256(raw.encode()).hexdigest()
            reference = repository + "@" + digest
            cls.command("docker", "pull", reference)
            cls.tags.append(reference)
            cls.release["images"].append(
                {
                    "artifact_key": key,
                    "repository": repository,
                    "digest": digest,
                    "digest_ref": reference,
                    "platforms": "linux/amd64",
                    "build_job": f"image:{key}:build",
                }
            )
        cls.seed_history()
        cls.compose("up", "-d", "--no-build", "--pull", "never", "--wait", "--wait-timeout", "180")
        cls.package = cls.root / "prepared-package"
        release_path = cls.root / "release.json"
        release_path.write_text(json.dumps(cls.release))
        cls.command(sys.executable, str(DEPLOY), "prepare", "--package", str(cls.package),
                    "--release", str(release_path), "--commit", cls.application_commit, "--pipeline-id", "1",
                    "--platform", "linux/amd64", "--signing-key", str(cls.key),
                    *(item for key, repository in cls.repositories.items() for item in ("--image", key + "=" + repository)),
                    timeout=300)

    @classmethod
    def cleanup(cls):
        if hasattr(cls, "root") and (cls.root / "docker-compose.yml").exists():
            diagnostic = cls.compose("logs", "--no-color", "postgres", check=False)
            path = cls.root / "fixture-postgres.private.log"
            path.write_text(diagnostic.stdout + diagnostic.stderr)
            path.chmod(0o600)
            cls.compose("down", "--volumes", "--remove-orphans", check=False)
        for tag in getattr(cls, "tags", []):
            cls.command("docker", "image", "rm", tag, check=False)
        # Private key and dumps remain in the untracked artifact directory;
        # workstation retention owns deletion, not a recursive cleanup script.

    @classmethod
    def sql(cls, statement: str, *, timeout: int = 180) -> str:
        return cls.compose(
            "exec",
            "-T",
            "postgres",
            "psql",
            "-X",
            "-v",
            "ON_ERROR_STOP=1",
            "-U",
            "piping",
            "-d",
            "piping",
            "-At",
            input=statement,
            timeout=timeout,
        ).stdout.strip()

    @classmethod
    def seed_history(cls):
        database_url = json.loads(cls.compose_bytes)["services"]["backend"]["environment"]["PIPING_DATABASE__URL"]
        cls.command("docker", "run", "--rm", "--pull", "never", "--network", cls.project + "_default",
                    "-e", "PIPING_START_MODE=migrate", "-e", "PIPING_REDIS__URL=redis://127.0.0.1:1",
                    "-e", "PIPING_SECRETS__CIPHER_KEY_B64=" + SYNTHETIC_CIPHER_KEY,
                    "-e", "PIPING_AUTH__PASSWORD_PEPPER=" + uuid.uuid4().hex,
                    "-e", "PIPING_AUTH__LOCAL_ENABLED=false",
                    "-e", "PIPING_DATABASE__URL=" + database_url, cls.prefix_image)
        cls.sql(f"INSERT INTO playbook(id,slug,name) VALUES ('{SCHEDULE}','native-transport','native transport');"
                f"INSERT INTO playbook_target_set(id,name,mode) VALUES ('{SCHEDULE}','native transport','static');"
                f"INSERT INTO playbook_service_principal(id,name,slug) VALUES ('{SCHEDULE}','native transport','native-transport');"
                "INSERT INTO playbook_schedule(id,playbook_id,target_set_id,service_principal_id,name,cron_expression,enabled) "
                f"VALUES ('{SCHEDULE}','{SCHEDULE}','{SCHEDULE}','{SCHEDULE}','native transport','0 * * * *',false);"
                f"INSERT INTO secrets_variable(id,key) VALUES ('{SCHEDULE}','secret-field/native-transport');"
                f"INSERT INTO secrets_store(secret_id,encrypted_value) VALUES ('{SCHEDULE}','conserved');")

    def deploy(self, action: str, stage: Path, *, release=None, extra=()):
        args = [
            sys.executable,
            str(DEPLOY),
            action,
            "--root",
            str(self.root),
            "--stage",
            str(stage),
            "--project-name",
            self.project,
        ]
        if action == "stage":
            package = self.package
            if release is not None:
                release_path = self.root / (stage.name + ".release.json")
                release_path.write_text(json.dumps(release))
                package = self.root / (stage.name + ".package")
                prepared = self.command(sys.executable, str(DEPLOY), "prepare", "--package", str(package),
                    "--release", str(release_path), "--commit", self.application_commit, "--pipeline-id", "1",
                    "--platform", "linux/amd64", "--signing-key", str(self.key),
                    *(item for key, repository in self.repositories.items() for item in ("--image", key + "=" + repository)),
                    check=False, timeout=300)
                if prepared.returncode:
                    return prepared
            args += [
                "--package", str(package), "--verification-key", str(self.public_key),
                "--disabled-schedule",
                SCHEDULE,
                "--image-file",
                self.image_file,
            ]
        return self.command(*args, *extra, check=False, timeout=300)

    def accept_applied_health(self, stage: Path):
        self.compose("up", "-d", "--no-build", "--pull", "never", "--no-recreate",
                     "--wait", "--wait-timeout", "180", "backend")
        result = self.deploy("verify-health", stage)
        self.assertEqual(result.returncode, 0, result.stderr)

    def writers(self):
        state = {}
        services = json.loads(self.compose("config", "--format", "json").stdout)["services"]
        for service in (name for name in ("backend", "ansible-runner", "proxy") if name in services):
            container = self.compose("ps", "-q", service).stdout.strip()
            self.assertTrue(container, service)
            info = json.loads(self.command("docker", "inspect", container).stdout)[0]
            self.assertTrue(info["State"]["Running"], service)
            self.command("docker", "exec", container, "sh", "-c", "kill -0 1")
            # An unchanged StartedAt and RestartCount detects stop/restart, too.
            state[service] = (
                container,
                info["State"]["StartedAt"],
                info["RestartCount"],
            )
        return state

    def stage_path(self, label):
        type(self).stage_sequence += 1
        stamp = datetime(2026, 10, 5, tzinfo=timezone.utc) + timedelta(seconds=self.stage_sequence)
        return self.root / "backups" / stamp.strftime("release-%Y%m%dT%H%M%SZ")

    def copied_dossier(self, stage, label):
        dossier = self.root / label
        dossier.mkdir(mode=0o700)
        request = json.loads((stage / "backup-request.json").read_bytes())
        files = ["plan.json", "backup-request.json", *request["files"], request["postgres_archive"]["archive"]]
        if request["installed_backend_archive"]:
            files.append(request["installed_backend_archive"]["archive"])
        for name in files:
            shutil.copyfile(stage / name, dossier / name)
            (dossier / name).chmod(0o600)
        return dossier, request

    def verify_dossier(self, dossier):
        return self.command(sys.executable, str(DEPLOY), "verify-backup", "--dossier", str(dossier),
                            "--package", str(self.package), "--signing-key", str(self.key),
                            "--verification-key", str(self.public_key), check=False, timeout=600)

class NativePublishedArtifactTests(NativeComposeFixture):
    """Carried genuine 23→24 artifacts; no image build and no historical approval."""

    application_commit = PRODUCER["PRODUCER_APPLICATION_REVISION"]
    postgres_reference = PRODUCER["POSTGRES_FIXTURE_IMAGE_ID"]

    @classmethod
    def setUpClass(cls):
        artifact_root = Path(os.environ["PIPING_NATIVE_ARTIFACT_ROOT"])
        cls.root = Path(tempfile.mkdtemp(prefix="published24-", dir=artifact_root))
        cls.root.chmod(0o700)
        cls.project, cls.tags, cls.stage_sequence = "published24-" + uuid.uuid4().hex[:12], [], 0
        cls.image_file = ".images.env"
        cls.addClassCleanup(cls.cleanup)
        inputs = Path(os.environ["PIPING_NATIVE_ARTIFACT_INPUTS"])
        cls.key, cls.public_key = inputs / "operator-fixture-signing.pem", inputs / "operator-fixture-verification.pem"
        cls.package = inputs / "prepared-package"
        package = deploy.verify_package(cls.package, cls.public_key)
        cls.release = json.loads((cls.package / "release.json").read_bytes())
        if package["commit"] != cls.application_commit or cls.release["version"] != PRODUCER["PRODUCER_VERSION"]:
            raise AssertionError("not the reviewed current producer")
        cls.repositories = {key: ref.rsplit("@", 1)[0] for key, ref in PRODUCER["PRODUCER_IMAGES"].items()}
        for role, expected in PRODUCER["PRODUCER_IMAGES"].items():
            if package["images"][role]["published_ref"] != expected:
                raise AssertionError("published role identity differs")
        inspection = cls.project + "-published-binary"
        cls.command("docker", "create", "--name", inspection, package["images"]["backend"]["local_id"])
        try:
            binary = cls.root / "published-piping-backend"
            cls.command("docker", "cp", inspection + ":/usr/local/bin/piping-backend", str(binary))
            if deploy.file_sha(binary) != PRODUCER["PRODUCER_BINARY_SHA256"]:
                raise AssertionError("published executable differs from producer receipt")
        finally:
            cls.command("docker", "rm", inspection)
        cls.prefix_image = deploy.load_image(inputs, json.loads((inputs / "installed23.json").read_bytes()),
                                             "linux/amd64", "7a54fe7a9c84cdd6016befc06589d1e91590e1b2")
        (cls.root / ".env").write_text("POSTGRES_USER=piping\nPOSTGRES_DB=piping\n")
        (cls.root / ".env").chmod(0o600)
        (cls.root / ".images.env").write_text("".join(
            f"PIPING_{key.upper().replace('-', '_')}_IMAGE={cls.prefix_image}\n" for key in KEYS))
        (cls.root / ".images.env").chmod(0o600)
        environment = {"PIPING_DATABASE__URL": "postgresql://piping@postgres:5432/piping",
                       "PIPING_REDIS__URL": "redis://redis:6379/0",
                       "PIPING_SECRETS__CIPHER_KEY_B64": SYNTHETIC_CIPHER_KEY,
                       "PIPING_AUTH__PASSWORD_PEPPER": uuid.uuid4().hex,
                       "PIPING_AUTH__LOCAL_ENABLED": "false",
                       "PIPING_SECURITY__ALLOW_WEAK_DEV_SECRETS": "true",
                       "PIPING_START_EMBEDDED_GUACD": "false",
                       "PIPING_OBSERVABILITY__OTEL_ENABLED": "false"}
        services = {
            "postgres": {"image": cls.postgres_reference, "cpus": 1, "mem_limit": "768m",
                         "environment": {"POSTGRES_USER": "piping", "POSTGRES_DB": "piping", "POSTGRES_HOST_AUTH_METHOD": "trust"},
                         "healthcheck": {"test": ["CMD", "pg_isready", "-U", "piping"], "interval": "1s", "timeout": "3s", "retries": 60}},
            "redis": {"image": "redis:8", "cpus": 0.5, "mem_limit": "128m"},
            "backend": {"image": "${PIPING_BACKEND_IMAGE}", "cpus": 1, "mem_limit": "1g", "environment": environment},
        }
        # Only migration/archives are accepted here, not frontend/runner health.
        # Their genuine carried images retain the same explicit idle command.
        for role in ("frontend", "ansible-runner"):
            services[role] = {"image": "${PIPING_" + role.upper().replace("-", "_") + "_IMAGE}",
                              "entrypoint": ["/bin/sh", "-c", "exec tail -f /dev/null"],
                              "healthcheck": {"disable": True},
                              "cpus": 0.5, "mem_limit": "128m"}
        cls.compose_bytes = json.dumps({"services": services}).encode()
        (cls.root / "docker-compose.yml").write_bytes(cls.compose_bytes)
        cls.compose("up", "-d", "--no-build", "--pull", "never", "--wait", "--wait-timeout", "120", "postgres", "redis")
        cls.seed_history()
        cls.sql("UPDATE playbook_schedule SET enabled=false;")
        cls.compose("up", "-d", "--no-build", "--pull", "never", "--wait", "--wait-timeout", "120")

    def test_published_23_to_24_and_empty_tail_carried_archive(self):
        self.assertEqual(self.sql("SELECT count(*) FROM _sqlx_migrations WHERE success"), "23")
        receipts = []
        for installed in (23, 24):
            stage = self.stage_path("published" + str(installed))
            result = self.deploy("stage", stage)
            self.assertEqual(result.returncode, 0, "stage failed; private artifact " + str(stage))
            result = self.deploy("backup", stage)
            self.assertEqual(result.returncode, 0, "backup failed; private artifact " + str(stage))
            dossier, request = self.copied_dossier(stage, "dossier" + str(installed))
            self.assertEqual(request["installed_backend_archive"] is None, installed == 24)
            result = self.verify_dossier(dossier)
            self.assertEqual(result.returncode, 0, "verify-backup failed; private artifact " + str(dossier))
            extra = ("--receipt", str(dossier / "restore-receipt.json"),
                     "--receipt-signature", str(dossier / "restore-receipt.sig"))
            result = self.deploy("apply", stage, extra=extra)
            self.assertEqual(result.returncode, 0, "apply failed; private artifact " + str(stage))
            self.assertEqual(self.sql("SELECT count(*) FROM _sqlx_migrations WHERE success"), "24")
            self.assertEqual(self.deploy("apply", stage, extra=extra).returncode, 2)
            receipts.append({"installed": installed, "dump_sha256": deploy.file_sha(stage / "offline-pre-upgrade.dump"),
                             "native_restore_migration_conservation": "passed"})
        deploy.write_json(self.root / "published-restore-receipt.json", {
            "published_ref": PRODUCER["PRODUCER_IMAGES"]["backend"], "application_source": self.application_commit,
            "backend_binary_sha256": PRODUCER["PRODUCER_BINARY_SHA256"], "postgres_image": self.postgres_reference,
            "helper_sha256": deploy.helper_identity(), "cohorts": receipts,
            "scope": "native archive/migration acceptance; no product startup or historical-target approval"})


class NativeComposeDeploymentTests(NativeComposeFixture):
    @classmethod
    def setUpClass(cls):
        # Failed-start cases retain containment; they cannot share a live graph.
        pass

    def setUp(self):
        type(self).initialize_fixture(self.addCleanup)

    def test_native_fixture_registry_postgres_packaged_source(self):
        self.writers()
        self.assertEqual(
            self.sql("SELECT count(*) FROM _sqlx_migrations WHERE success;"),
            str(len(list((ROOT / "apps/api/migrations").glob("*.sql")))),
        )
        for source in (ROOT / "apps/api/migrations").glob("*.sql"):
            version = int(source.name.split("_", 1)[0])
            self.assertEqual(
                self.sql(f"SELECT encode(checksum, 'hex') FROM _sqlx_migrations WHERE version={version};"),
                hashlib.sha384(source.read_bytes()).hexdigest(),
            )
        name = self.project + "-packaged-source"
        self.command("docker", "create", "--name", name, self.release["images"][0]["digest_ref"])
        try:
            packaged = self.root / "extracted"
            self.command("docker", "cp", name + ":/app/migrations", str(packaged))
            self.assertEqual(
                {p.name: p.read_bytes() for p in packaged.glob("*.sql")},
                {p.name: p.read_bytes() for p in (ROOT / "apps/api/migrations").glob("*.sql")},
            )
            if os.environ.get("PIPING_NATIVE_SOURCE_SHA"):
                paired_converter = self.root / "packaged-convert-fhrp-auth-keys"
                self.command("docker", "cp", name + ":/usr/local/bin/convert-fhrp-auth-keys", str(paired_converter))
                self.assertEqual(deploy.file_sha(paired_converter), os.environ["PIPING_NATIVE_CONVERTER_SHA256"])
        finally:
            self.command("docker", "rm", name)

    def test_native_manifest_digest_is_not_config_id(self):
        image = self.release["images"][0]
        inspected = json.loads(self.command("docker", "image", "inspect", image["digest_ref"]).stdout)[0]
        manifest = json.loads(
            self.command(
                "docker",
                "buildx",
                "imagetools",
                "inspect",
                "--raw",
                image["digest_ref"],
            ).stdout
        )
        # Docker 29's containerd store can report the manifest as inspect.Id;
        # OCI config identity is the manifest's config.digest, never guessed
        # from that engine-dependent field.
        self.assertNotEqual(manifest["config"]["digest"], image["digest"])
        self.assertIn(image["digest_ref"], inspected["RepoDigests"])
        self.assertEqual(inspected["Config"]["Labels"]["org.opencontainers.image.revision"], self.application_commit)
        before = self.writers()
        result = self.deploy("stage", self.stage_path("manifest"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.writers(), before)

    def test_invalid_release_boundaries_preserve_live_writers(self):
        for boundary in ("repository", "platform", "revision"):
            with self.subTest(boundary=boundary):
                release = copy.deepcopy(self.release)
                if boundary == "repository":
                    release["images"][0]["repository"] += "-wrong"
                else:
                    # Publish genuine wrong-platform / wrong-revision images;
                    # leave the bundle's claimed platform at linux/amd64 so
                    # validation must reach native registry/engine metadata.
                    context = self.root / "context"
                    tag = self.repositories["backend"] + ":wrong-" + boundary
                    self.tags.append(tag)
                    platform, revision = "linux/amd64", "d" * 40
                    dockerfile = context / "Dockerfile"
                    if boundary == "platform":
                        platform, revision = "linux/arm64", self.application_commit
                        dockerfile = context / "Dockerfile.arm64"
                        dockerfile.write_text(
                            "FROM scratch\nARG REVISION\n"
                            "LABEL org.opencontainers.image.revision=$REVISION\n"
                            'COPY migrations /app/migrations\nCMD ["/fixture-not-started"]\n'
                        )
                    self.command(
                        "docker",
                        "buildx",
                        "build",
                        "--platform",
                        platform,
                        "--provenance=false",
                        "--build-arg",
                        "REVISION=" + revision,
                        "--push",
                        "-f",
                        str(dockerfile),
                        "-t",
                        tag,
                        str(context),
                        timeout=300,
                    )
                    raw = self.command("docker", "buildx", "imagetools", "inspect", "--raw", tag).stdout
                    digest = "sha256:" + hashlib.sha256(raw.encode()).hexdigest()
                    release["images"][0].update(
                        digest=digest,
                        digest_ref=self.repositories["backend"] + "@" + digest,
                    )
                before = self.writers()
                result = self.deploy("stage", self.stage_path(boundary), release=release)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.writers(), before)

    def test_foreign_sqlx_history_denies_backup_before_writer_stop(self):
        latest_version = int(self.sql("SELECT max(version) FROM _sqlx_migrations;"))
        foreign_version = latest_version + 1
        for case in ("checksum", "unsuccessful", "extra-version"):
            with self.subTest(case=case):
                stage = self.stage_path(case)
                result = self.deploy("stage", stage)
                self.assertEqual(result.returncode, 0, result.stderr)
                original = self.sql(
                    f"SELECT encode(checksum, 'hex') FROM _sqlx_migrations WHERE version={latest_version};"
                )
                if case == "checksum":
                    self.sql(f"UPDATE _sqlx_migrations SET checksum=decode('00', 'hex') WHERE version={latest_version};")
                elif case == "unsuccessful":
                    self.sql(f"UPDATE _sqlx_migrations SET success=false WHERE version={latest_version};")
                else:
                    self.sql(
                        f"INSERT INTO _sqlx_migrations VALUES ({foreign_version}, 'foreign', now(), true, decode('00','hex'),0);"
                    )

                try:
                    before = self.writers()
                    result = self.deploy("backup", stage)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(self.writers(), before)
                    self.assertFalse((stage / "offline-pre-upgrade.dump").exists())
                finally:
                    self.sql(
                        f"DELETE FROM _sqlx_migrations WHERE version={foreign_version}; "
                        f"UPDATE _sqlx_migrations SET success=true, checksum=decode('{original}', 'hex') WHERE version={latest_version};"
                    )

    def test_unrecognized_schema_drift_is_refused_before_writer_stop(self):
        before = self.writers()
        self.sql("CREATE DOMAIN public.native_undeclared_domain AS text;")
        try:
            result = self.deploy("stage", self.stage_path("unknown-domain"))
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(self.writers(), before)
        finally:
            self.sql("DROP DOMAIN public.native_undeclared_domain;")

    def test_disabled_user_and_internal_fk_triggers_deny_stage_without_writer_stop(self):
        internal = json.loads(self.sql(
            "SELECT json_build_object('schema',n.nspname,'table',c.relname,'trigger',t.tgname) "
            "FROM pg_trigger t JOIN pg_constraint k ON k.oid=t.tgconstraint "
            "JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace "
            "WHERE t.tgisinternal AND k.contype='f' AND n.nspname='public' "
            "ORDER BY c.relname,k.conname,t.tgtype LIMIT 1;"
        ))
        targets = [("public", "playbook_version", "trg_playbook_version_immutable"),
                   (internal["schema"], internal["table"], internal["trigger"])]
        for schema, table, trigger in targets:
            relation = ".".join('"' + part.replace('"', '""') + '"' for part in (schema, table))
            identifier = '"' + trigger.replace('"', '""') + '"'
            with self.subTest(internal=trigger != "trg_playbook_version_immutable"):
                before = self.writers()
                self.sql("ALTER TABLE " + relation + " DISABLE TRIGGER " + identifier + ";")
                try:
                    stage = self.stage_path("disabled-trigger")
                    result = self.deploy("stage", stage)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(self.writers(), before)
                    self.assertFalse((stage / "offline-pre-upgrade.dump").exists())
                    self.assertFalse((stage / "journal/03-writers-stopping").exists())
                finally:
                    self.sql("ALTER TABLE " + relation + " ENABLE TRIGGER " + identifier + ";")

    def test_operator_carried_backup_receipt_apply_and_replay_conserve_data(self):
        stage = self.stage_path("offline")
        for image in self.release["images"]:
            self.command("docker", "image", "rm", "-f", image["digest_ref"])
        self.compose("stop", "registry")
        try:
            candidate = self.release["images"][0]["digest_ref"]
            self.assertNotEqual(self.command("docker", "image", "inspect", candidate, check=False).returncode, 0)
            self.assertNotEqual(self.command("docker", "pull", candidate, check=False, timeout=30).returncode, 0)
            result = self.deploy("stage", stage)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = self.deploy("backup", stage)
            journal = stage / "journal"
            phases = deploy.PREPARATION_PHASES + deploy.PHASES
            completed = []
            if journal.is_dir():
                for path in sorted(journal.iterdir()):
                    match = re.fullmatch(r"[0-9]{2}-(.+)", path.name)
                    if match and match[1] in phases and path.is_file():
                        completed.append(match[1])
            self.native_failure_context = {
                "backup_exit": result.returncode,
                "backup_phase": completed[-1] if completed else "none",
            }
            failure_records = list(stage.glob("failure-*.json"))
            if (len(failure_records) == 1
                    and re.fullmatch(r"[0-9a-f]{40}", os.environ.get("PIPING_NATIVE_SOURCE_SHA", ""))):
                try:
                    failure = deploy.read_json(failure_records[0])
                    self.native_failure_context.update(deploy.validated_owner_location(failure))
                    self.native_failure_context.update(deploy.validated_failure_context(failure))
                except (OSError, ValueError, TypeError):
                    # Diagnostics are optional and must not mask the backup refusal.
                    pass
            self.assertEqual(result.returncode, 0,
                             f"backup exit={result.returncode}; journal={completed}; "
                             "native command stderr is in private stage diagnostics")
            del self.native_failure_context
            dossier, request = self.copied_dossier(stage, "leader-dossier")
            bad_dossier = self.root / "leader-corrupt-dossier"
            shutil.copytree(dossier, bad_dossier)
            bad_dossier.chmod(0o700)
            bad_dump = bad_dossier / "offline-pre-upgrade.dump"
            bad_dump.write_bytes(b"not a PostgreSQL archive")
            bad_request = copy.deepcopy(request)
            bad_request["files"][bad_dump.name] = {"sha256": deploy.file_sha(bad_dump), "size": bad_dump.stat().st_size}
            (bad_dossier / "backup-request.json").write_text(json.dumps(bad_request))
            result = self.verify_dossier(bad_dossier)
            self.assertEqual(result.returncode, 2)
            self.assertFalse((bad_dossier / "restore-receipt.sig").exists())
            result = self.verify_dossier(dossier)
            self.assertEqual(result.returncode, 0, result.stderr)
            extra = ("--receipt", str(dossier / "restore-receipt.json"),
                     "--receipt-signature", str(dossier / "restore-receipt.sig"))
            # The current source-built backend already owns this dump/restore
            # proof. Published old22/23/24 migrators belong only to their pinned
            # historical routes and cannot consume a consolidated ledger.
            before = (self.root / self.image_file).read_bytes()
            # All denial cases precede receipt consumption and image installation.
            # Restore the exact baseline after each deliberate local drift.
            env_path = self.root / ".env"
            env_original = env_path.read_bytes()
            env_path.write_bytes(env_original + b"# drift after fresh backup\n")
            self.assertEqual(self.deploy("apply", stage, extra=extra).returncode, 2)
            self.assertEqual((self.root / self.image_file).read_bytes(), before)
            env_path.write_bytes(env_original)
            drift_id = "00000000-0000-4000-8000-000000000098"
            self.sql("INSERT INTO playbook(id,slug,name) VALUES ('" + drift_id + "','drift-after-backup','drift after backup');")
            self.assertEqual(self.deploy("apply", stage, extra=extra).returncode, 2)
            self.assertEqual((self.root / self.image_file).read_bytes(), before)
            self.sql("DELETE FROM playbook WHERE id='" + drift_id + "';")
            self.sql("ALTER TABLE playbook_version DISABLE TRIGGER trg_playbook_version_immutable;")
            self.assertEqual(self.deploy("apply", stage, extra=extra).returncode, 2)
            self.assertEqual((self.root / self.image_file).read_bytes(), before)
            self.sql("ALTER TABLE playbook_version ENABLE TRIGGER trg_playbook_version_immutable;")
            self.assertFalse((stage / "consumed-receipt.json").exists())
            self.assertFalse(set(deploy.WRITERS).intersection(self.compose("ps", "--services", "--status", "running").stdout.split()))
            signature = dossier / "restore-receipt.sig"
            original = signature.read_bytes()
            signature.write_bytes(b"forged")
            result = self.deploy("apply", stage, extra=extra)
            self.assertEqual(result.returncode, 2)
            self.assertEqual((self.root / self.image_file).read_bytes(), before)
            self.assertFalse(set(deploy.WRITERS).intersection(self.compose("ps", "--services", "--status", "running").stdout.split()))
            signature.write_bytes(original)
            result = self.deploy("apply", stage, extra=extra)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.sql("SELECT count(*) FROM _sqlx_migrations WHERE success;"),
                             str(len(list((ROOT / "apps/api/migrations").glob("*.sql")))))
            self.assertEqual(self.sql("SELECT NOT enabled FROM playbook_schedule WHERE id='" + SCHEDULE + "';"), "t")
            self.assertTrue((stage / "consumed-receipt.json").exists())
            result = self.deploy("apply", stage, extra=extra)
            self.assertEqual(result.returncode, 2)
            self.accept_applied_health(stage)
            # The already-current ledger uses the same protocol with no prefix archive.
            # Select Printum's .env-only consumer and omit the optional proxy.
            self.compose("stop", "proxy")
            self.compose("rm", "-f", "proxy")
            topology = json.loads((self.root / "docker-compose.yml").read_bytes())
            del topology["services"]["proxy"]
            (self.root / "docker-compose.yml").write_text(json.dumps(topology))
            env_path.write_bytes(env_original + (self.root / ".images.env").read_bytes())
            type(self).image_file = ".env"
            noop = self.stage_path("baseline-current")
            self.assertEqual(self.deploy("stage", noop).returncode, 0)
            self.assertEqual(self.deploy("backup", noop).returncode, 0)
            noop_dossier, noop_request = self.copied_dossier(noop, "leader-baseline-current")
            self.assertIsNone(noop_request["installed_backend_archive"])
            result = self.verify_dossier(noop_dossier)
            self.assertEqual(result.returncode, 0, result.stderr)
            noop_extra = ("--receipt", str(noop_dossier / "restore-receipt.json"),
                          "--receipt-signature", str(noop_dossier / "restore-receipt.sig"))
            untouched_images_file = (self.root / ".images.env").read_bytes()
            self.assertEqual(self.deploy("apply", noop, extra=noop_extra).returncode, 0)
            self.assertEqual((self.root / ".images.env").read_bytes(), untouched_images_file)
            self.accept_applied_health(noop)
            # A real missing bind source makes Docker fail midway through startup.
            failed_start = self.stage_path("writer-failure")
            self.assertEqual(self.deploy("stage", failed_start).returncode, 0)
            self.assertEqual(self.deploy("backup", failed_start).returncode, 0)
            failure_dossier, _ = self.copied_dossier(failed_start, "leader-writer-failure")
            self.assertEqual(self.verify_dossier(failure_dossier).returncode, 0)
            self.start_dependency.unlink()
            failure_extra = ("--receipt", str(failure_dossier / "restore-receipt.json"),
                             "--receipt-signature", str(failure_dossier / "restore-receipt.sig"))
            result = self.deploy("apply", failed_start, extra=failure_extra)
            self.assertEqual(result.returncode, 2)
            self.assertFalse(set(deploy.WRITERS).intersection(self.compose("ps", "--services", "--status", "running").stdout.split()))
            self.assertEqual(self.sql("SELECT count(*) FROM _sqlx_migrations WHERE success;"),
                             str(len(list((ROOT / "apps/api/migrations").glob("*.sql")))))
            deploy.write_json(self.root / "native-flow-receipt.json", {
                "schema": "piping.compose-native-flow/v1", "helper_sha256": deploy.helper_identity(),
                 "synthetic_transport_cohort": True, "native_migrator_proved": True,
                 "application_source": self.application_commit,
                "candidate_absent_before_stage": True, "registry_unavailable_before_stage": True,
                "restore_failure_rejected": True, "receipt_forgery_rejected": True, "replay_rejected": True,
                 "configuration_database_trigger_drift_rejected": True, "env_only_baseline": True,
                "optional_proxy_absent": True, "failed_start_writers_stopped": True,
                  "ledger_versions": len(list((ROOT / "apps/api/migrations").glob("*.sql")))})
            # Containment remains intact until this disposable graph is cleaned up.
        finally:
            self.compose("up", "-d", "--no-deps", "--pull", "never", "--no-build", "registry")


class NativeHistoricalPreservationTests(NativeComposeFixture):
    application_commit = PRODUCER["APPLICATION_REVISION"]
    postgres_reference = "sha256:54451ecb8ab38c24c3ec123f2fd501303a3a1856a5c66e98cecf2460d5e1e9d7"

    @classmethod
    def setUpClass(cls):
        # This fixture reports an immutable old22 capture. Building it from the
        # consolidated baseline would fabricate that oracle and its receipts.
        if not (ROOT / "apps/api/migrations/0022_virtual_machine_type_finite_vcpus.sql").is_file():
            raise AssertionError("Historical CHECK oracle requires its pinned old22 source export; "
                                 "the consolidated baseline cannot substitute for historical fixtures")
        super().setUpClass()

    @classmethod
    def seed_history(cls):
        super().seed_history()
        cls.canonical_catalog = deploy.catalog_objects(cls.sql(deploy.CATALOG_SQL))
        source_ports_tree = cls.sql("SELECT conbin::text FROM pg_constraint WHERE "
                                   "conrelid='public.inventory_dataflow'::regclass "
                                   "AND conname='inventory_dataflow_destination_ports_check'")
        original_data = cls.compose("exec", "-T", "postgres", "pg_dump", "--data-only", "--column-inserts",
                                    "-U", "piping", "-d", "piping").stdout
        dump = cls.compose("exec", "-T", "postgres", "pg_dump", "--schema-only", "--no-owner", "--no-privileges",
                           "-U", "piping", "-d", "piping").stdout
        tables = {name.rsplit(".", 1)[0] for name in schema_contract.historical_check_forms()}
        candidates = sorted({o["name"].rsplit(".", 1)[0] for o in cls.canonical_catalog if o["kind"] == "column"})
        for name in candidates:
            if len(tables) == 11:
                break
            tables.add(name)
        cls.layout_tables = sorted(tables)
        for table in cls.layout_tables:
            pattern = r"CREATE TABLE " + re.escape(table) + r" \(\n(.*?)\n\);"
            match = re.search(pattern, dump, re.S)
            if match is None:
                raise AssertionError("synthetic historical table missing from native schema dump")
            lines = match[1].splitlines()
            column_indices = [i for i, line in enumerate(lines) if line.startswith("    ") and not line.lstrip().startswith("CONSTRAINT ")]
            if len(column_indices) >= 2:
                first, second = column_indices[:2]
                lines[first], lines[second] = lines[second], lines[first]
            body = "    _historical_gap integer,\n" + "\n".join(lines)
            dump = dump[:match.start(1)] + body + dump[match.end(1):]
        cls.compose("exec", "-T", "postgres", "psql", "-X", "-v", "ON_ERROR_STOP=1", "-U", "piping", "-d", "postgres",
                    input="DROP DATABASE piping WITH (FORCE); CREATE DATABASE piping OWNER piping;")
        cls.compose("exec", "-T", "postgres", "psql", "-X", "-v", "ON_ERROR_STOP=1", "-U", "piping", "-d", "piping", input=dump)
        # Only this disposable database receives historical physical slots and a
        # historical stored tree. No original customer capture is a git fixture.
        for table in cls.layout_tables:
            cls.sql("ALTER TABLE " + table + " DROP COLUMN _historical_gap;")
        cls.compose("exec", "-T", "postgres", "psql", "-X", "-v", "ON_ERROR_STOP=1", "-U", "piping", "-d", "piping",
                    input=original_data)
        cls.sql("CREATE SCHEMA synthetic_preserved;")
        for table in range(8):
            width = 15 if table == 7 else 16
            cls.sql("CREATE TABLE synthetic_preserved.retained_" + str(table) + " ("
                    + ",".join("value_" + str(column) + " text" for column in range(width)) + ");")
            cls.sql("INSERT INTO synthetic_preserved.retained_" + str(table)
                    + " (value_0,value_1) VALUES ('opaque-private-cipher','same'),('opaque-private-cipher','same');")
        for name, form in schema_contract.historical_check_forms().items():
            relation, constraint = name.rsplit(".", 1)
            cls.sql("ALTER TABLE " + relation + " DROP CONSTRAINT " + constraint + "; ALTER TABLE " + relation
                    + " ADD CONSTRAINT " + constraint + " " + form.historical_sql + ";")
        pg = {"compose": ["docker", "compose", "--project-name", cls.project, "--project-directory", str(cls.root),
                         "--env-file", str(cls.root / ".env"), "--env-file", str(cls.root / ".images.env"),
                         "-f", str(cls.root / "docker-compose.yml")]}
        owner = deploy.Deployment.__new__(deploy.Deployment)
        form = schema_contract.historical_check_forms()["public.playbook_artifact.chk_playbook_artifact_size"]
        binding = next(b for b in json.loads(cls.sql(schema_contract.CHECK_BINDINGS_SQL % "'public.playbook_artifact'"))
                       if b["name"] == form.name)
        with owner.native_sql_session(pg) as sql:
            declarations = []
            slots = {column["position"]: column for column in binding["columns"]}
            for position in range(1, max(slots) + 1):
                column = slots.get(position)
                declarations.append('"' + column["name"] + '" ' + column["type"] if column else
                                    '"_proof_gap_' + str(position) + '" integer')
            sql("CREATE TEMP TABLE pwa_check_proof (" + ",".join(declarations) + ",CONSTRAINT canonical "
                + form.canonical_sql + ",CONSTRAINT historical " + form.historical_sql
                + "," + ",".join("CONSTRAINT proof_atom_" + str(i) + " CHECK (" + atom + ")" for i, atom in enumerate(form.atoms))
                + ",CONSTRAINT proof_pair CHECK (" + " AND ".join(form.atoms[:2]) + "))")
            references = json.loads(sql(schema_contract.CHECK_BINDINGS_SQL % "'pg_temp.pwa_check_proof'"))
            tree = schema_contract._associated_source_ast(references, form)
            # pg_node_tree uses the native text Datum storage but rejects SQL text
            # input. This disposable-only internal constructor stores a tree made
            # exclusively from this server's compiled source atom/pair nodes.
            sql("CREATE FUNCTION pg_temp.store_native_tree(cstring) RETURNS pg_node_tree LANGUAGE internal IMMUTABLE AS 'textin'")
            sql("UPDATE pg_constraint SET conbin=pg_temp.store_native_tree('" + source_ports_tree.replace("'", "''")
                + "') WHERE conrelid='public.inventory_dataflow'::regclass AND conname='inventory_dataflow_destination_ports_check'")
            sql("UPDATE pg_constraint SET conbin=pg_temp.store_native_tree('" + tree.replace("'", "''")
                + "') WHERE conrelid='public.playbook_artifact'::regclass AND conname='chk_playbook_artifact_size'")

    def native_capture(self):
        owner = deploy.Deployment.__new__(deploy.Deployment)
        owner.sql = lambda query, **kwargs: self.sql(query, **kwargs).encode()
        bindings = owner.installed_check_bindings()
        forms = "(" + ",".join("'" + name + "'" for name in schema_contract.historical_check_forms()) + ")"
        capture = json.loads(self.sql(schema_contract.HISTORICAL_BUILTINS_SQL))
        capture.update({"postgres_version": "18.3", "images": {"postgres": self.postgres_reference},
                        "ledger": {"count": 22, "max": 22, "successful": 22}, "bindings": bindings})
        capture["original"] = json.loads(self.sql("SELECT json_agg(json_build_object('name',n.nspname||'.'||c.relname||'.'||k.conname,"
            "'conkey',k.conkey,'enforced',k.conenforced,'validated',k.convalidated,'relation_oid',k.conrelid::text,"
            "'namespace_oid',k.connamespace::text,'constraint_oid',k.oid::text,'original_conbin',k.conbin::text)) "
            "FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace "
            "WHERE n.nspname||'.'||c.relname||'.'||k.conname IN " + forms))
        capture["dependencies"] = json.loads(self.sql("SELECT json_agg(json_build_object('name',n.nspname||'.'||c.relname||'.'||k.conname,"
            "'objid',d.objid::text,'classid',d.classid::text,'deptype',d.deptype,'objsubid',d.objsubid,"
            "'refobjid',d.refobjid::text,'refclassid',d.refclassid::text,'description',pg_describe_object(d.refclassid,d.refobjid,d.refobjsubid),"
            "'refobjsubid',d.refobjsubid)) FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace "
            "JOIN pg_depend d ON d.classid='pg_constraint'::regclass AND d.objid=k.oid "
            "WHERE d.refclassid='pg_class'::regclass AND d.refobjid=k.conrelid AND n.nspname||'.'||c.relname||'.'||k.conname IN " + forms))
        return capture

    def test_historical_target_requires_exact_signed_preservation(self):
        self.assertEqual(len(self.layout_tables), 11)
        original = deploy.catalog_objects(self.sql(deploy.CATALOG_SQL))
        canonical = {(o["kind"], o["name"]): o for o in self.canonical_catalog}
        inventory = [{"kind": o["kind"], "name": o["name"], "definition_sha256": schema_contract.definition_hash(o["definition"])}
                     for o in original if (o["kind"], o["name"]) not in canonical]
        self.assertEqual(len(inventory), 136)
        before = self.writers()
        self.assertEqual(self.deploy("stage", self.stage_path("no-approval")).returncode, 2)
        self.assertEqual(self.writers(), before)
        capture = self.native_capture()
        capture_path = self.root / "synthetic-check-capture.json"
        capture_path.write_text(json.dumps(capture))
        capture_path.chmod(0o600)
        metadata = deploy.Deployment.__new__(deploy.Deployment)
        metadata.stage = self.root
        metadata.plan = {"contract": json.loads((ROOT / "apps/api/migrations/compose-deployment.json").read_bytes())}
        metadata.sql = lambda query, **kwargs: self.sql(query, **kwargs).encode()
        package = json.loads((self.package / "package.json").read_bytes())
        history = json.loads(self.sql("SELECT json_agg(row_to_json(h) ORDER BY version::bigint) FROM "
            "(SELECT version::text,description,installed_on AT TIME ZONE 'UTC' AS installed_on,success,"
            "encode(checksum,'hex') AS checksum,execution_time FROM _sqlx_migrations) h"))
        migrations = {"apps/api/migrations/" + p.name: hashlib.sha384(p.read_bytes()).hexdigest()
                      for p in (ROOT / "apps/api/migrations").glob("*.sql") if int(p.name.split('_', 1)[0]) <= 22}
        now = datetime.now(timezone.utc)
        manifest = {"schema": "piping.compose-preservation/v1", "status": "approved", "context": {
            "target": socket.gethostname(), "root": str(self.root), "project_name": self.project,
            "application_source": self.application_commit, "helper_sha256": deploy.helper_identity(),
            "release_sha256": package["release_sha256"], "postgres_image": self.postgres_reference,
            "backend_image": json.loads(self.command("docker", "image", "inspect", self.prefix_image).stdout)[0]["Id"],
            "ledger_sha256": schema_contract.definition_hash(history), "prefix_sha256": schema_contract.definition_hash(migrations),
            "installed_catalog_sha256": schema_contract.definition_hash(original)},
            "package_binding_sha256": package_binding(package), "issued_at": now.isoformat().replace('+00:00','Z'),
            "expires_at": (now + timedelta(hours=24)).isoformat().replace('+00:00','Z'), "inventory": inventory,
            "position_pairs": {o["name"]: [canonical[(o["kind"], o["name"])]["definition"]["position"], o["definition"]["position"]]
                               for o in original if o["kind"] == "column" and (o["kind"], o["name"]) in canonical
                               and o["definition"]["position"] != canonical[(o["kind"], o["name"])]["definition"]["position"]},
            "check_capture_sha256": deploy.file_sha(capture_path), "append_slots": metadata.append_slots()}
        approval_path = self.root / "synthetic-operator-approval.json"
        approval_path.write_text(json.dumps(manifest))
        approval_path.chmod(0o600)
        prepared = self.root / "historical-approved-package"
        result = self.command(sys.executable, str(DEPLOY), "prepare", "--package", str(prepared),
            "--release", str(self.root / "release.json"), "--commit", self.application_commit, "--pipeline-id", "1",
            "--platform", "linux/amd64", "--signing-key", str(self.key), "--preservation-manifest", str(approval_path),
            "--check-capture", str(capture_path),
            *(item for key, repository in self.repositories.items() for item in ("--image", key + "=" + repository)), check=False, timeout=600)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.package = prepared
        approved_package = self.package
        def signed_variant(changed, label):
            directory = self.root / ("approval-variant-" + label)
            shutil.copytree(approved_package, directory, copy_function=os.link)
            directory.chmod(0o700)
            for filename in ("preservation.json", "package.json", "package.sig"):
                (directory / filename).unlink()
            deploy.write_json(directory / "preservation.json", changed)
            body = json.loads((approved_package / "package.json").read_bytes())
            body["preservation"]["manifest_sha256"] = deploy.file_sha(directory / "preservation.json")
            deploy.write_json(directory / "package.json", body)
            deploy.sign(directory / "package.json", self.key, directory / "package.sig")
            return directory
        variants = []
        bad = copy.deepcopy(manifest)
        bad["inventory"].pop()
        variants.append(("missing-inventory", bad))
        bad = copy.deepcopy(manifest)
        bad["inventory"][0]["definition_sha256"] = "0" * 64
        variants.append(("changed-definition", bad))
        bad = copy.deepcopy(manifest)
        bad["position_pairs"].pop(next(iter(bad["position_pairs"])))
        variants.append(("missing-position", bad))
        bad = copy.deepcopy(manifest)
        bad["context"]["target"] = "wrong-target"
        variants.append(("wrong-target", bad))
        bad = copy.deepcopy(manifest)
        bad["issued_at"] = (now-timedelta(days=2)).isoformat().replace('+00:00','Z')
        bad["expires_at"] = (now-timedelta(days=1)).isoformat().replace('+00:00','Z')
        variants.append(("expired", bad))
        for label, changed in variants:
            self.package = signed_variant(changed, label)
            result = self.deploy("stage", self.stage_path(label))
            self.assertEqual(result.returncode, 2)
            self.assertEqual(self.writers(), before)
        self.package = approved_package
        self.sql("ALTER TABLE synthetic_preserved.retained_0 ADD COLUMN unlisted text;")
        self.assertEqual(self.deploy("stage", self.stage_path("unlisted-inside-approved-relation")).returncode, 2)
        self.assertEqual(self.writers(), before)
        # Existing visible attributes retain their exact positions after removal.
        self.sql("ALTER TABLE synthetic_preserved.retained_0 DROP COLUMN unlisted;")
        stage = self.stage_path("historical")
        result = self.deploy("stage", stage)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.deploy("backup", stage).returncode, 0)
        approval_bytes = (stage / "preservation.json").read_bytes()
        approval_signature = (stage / "package.sig").read_bytes()
        self.sql("ALTER TABLE synthetic_preserved.retained_0 ADD COLUMN drift_after_backup text;")
        self.assertEqual(self.deploy("apply", stage, extra=("--receipt", str(self.root / "not-issued.json"),
                                                           "--receipt-signature", str(self.root / "not-issued.sig"))).returncode, 2)
        self.assertFalse((stage / "consumed-receipt.json").exists())
        self.sql("ALTER TABLE synthetic_preserved.retained_0 DROP COLUMN drift_after_backup;")
        dossier, request = self.copied_dossier(stage, "historical-leader-dossier")
        result = self.verify_dossier(dossier)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((stage / "preservation.json").read_bytes(), approval_bytes)
        self.assertEqual((stage / "package.sig").read_bytes(), approval_signature)
        extra = ("--receipt", str(dossier / "restore-receipt.json"), "--receipt-signature", str(dossier / "restore-receipt.sig"))
        result = self.deploy("apply", stage, extra=extra)
        self.assertEqual(result.returncode, 0, result.stderr)
        final = deploy.catalog_objects(self.sql(deploy.CATALOG_SQL))
        plan = json.loads((stage / "plan.json").read_bytes())
        schema_contract.exact_target_preservation(original, final, additions=plan["installed_candidate_additions"],
                                                  replacements=plan["installed_candidate_replacements"])
        self.assertEqual(self.sql("SELECT count(*) FROM _sqlx_migrations WHERE success"), "24")
        # A fresh approval binds the current ledger, image and physical slots.
        # The historical capture remains the original 22 oracle, not a recapture.
        original_approval_bytes = approval_path.read_bytes()
        manifest24 = copy.deepcopy(manifest)
        now24 = datetime.now(timezone.utc)
        manifest24.update(issued_at=now24.isoformat().replace('+00:00', 'Z'),
                          expires_at=(now24 + timedelta(hours=24)).isoformat().replace('+00:00', 'Z'))
        canonical24 = {(o["kind"], o["name"]): o for o in
                       schema_contract.migration_catalog(self.canonical_catalog, **plan["source_delta"])}
        manifest24["context"].update({
            "backend_image": plan["images"]["backend"]["local_id"],
            "ledger_sha256": schema_contract.definition_hash(json.loads(self.sql(
                "SELECT json_agg(row_to_json(h) ORDER BY version::bigint) FROM "
                "(SELECT version::text,description,installed_on AT TIME ZONE 'UTC' AS installed_on,success,"
                "encode(checksum,'hex') AS checksum,execution_time FROM _sqlx_migrations) h"))),
            "installed_catalog_sha256": schema_contract.definition_hash(final)})
        manifest24["append_slots"] = metadata.append_slots()
        manifest24["position_pairs"] = {o["name"]: [canonical24[(o["kind"], o["name"])]["definition"]["position"], o["definition"]["position"]]
            for o in final if o["kind"] == "column" and (o["kind"], o["name"]) in canonical24
            and o["definition"]["position"] != canonical24[(o["kind"], o["name"])]["definition"]["position"]}
        approval24_path = self.root / "synthetic-operator-approval24.json"
        deploy.write_json(approval24_path, manifest24)
        self.assertEqual(approval_path.read_bytes(), original_approval_bytes)
        prepared24 = self.root / "historical-approved-package24"
        result = self.command(sys.executable, str(DEPLOY), "prepare", "--package", str(prepared24),
            "--release", str(self.root / "release.json"), "--commit", self.application_commit, "--pipeline-id", "1",
            "--platform", "linux/amd64", "--signing-key", str(self.key), "--preservation-manifest", str(approval24_path),
            "--check-capture", str(capture_path),
            *(item for key, repository in self.repositories.items() for item in ("--image", key + "=" + repository)), check=False, timeout=600)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.package = prepared24
        noop = self.stage_path("historical24")
        self.assertEqual(self.deploy("stage", noop).returncode, 0)
        self.assertEqual(json.loads((noop / "plan.json").read_bytes())["installed_candidate_additions"], [])
        self.assertEqual(self.deploy("backup", noop).returncode, 0)
        dossier24, request24 = self.copied_dossier(noop, "historical-leader-dossier24")
        self.assertNotEqual(request24["nonce"], request["nonce"])
        result = self.verify_dossier(dossier24)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.deploy("apply", noop, extra=("--receipt", str(dossier24 / "restore-receipt.json"),
                                                  "--receipt-signature", str(dossier24 / "restore-receipt.sig")))
        self.assertEqual(result.returncode, 0, result.stderr)
        schema_contract.exact_target_preservation(final, deploy.catalog_objects(self.sql(deploy.CATALOG_SQL)))
        deploy.write_json(self.root / "historical-preservation-public.json", {"schema": "piping.compose-preservation-proof/v1",
            "helper_sha256": deploy.helper_identity(), "application_source": self.application_commit,
            "backend_binary_sha256": deploy.file_sha(Path(os.environ["PIPING_NATIVE_BACKEND_BINARY"])),
            "backend_image": PRODUCER["BACKEND_IMAGE"],
            "prefix_sha256": schema_contract.definition_hash(migrations), "postgres_image": self.postgres_reference,
            "successful_24_empty_tail_restore_apply": True,
            "approved_objects": len(inventory), "synthetic_relations": 8, "physical_layout_tables": len(self.layout_tables),
            "native_check_bindings": len(capture["bindings"]), "native_leader_restore_and_apply": "passed",
            "all12_native_checks_including_nested_and": True,
            "all_preexisting_objects_exact": True, "all_old_column_multisets_conserved": True,
            "sequences_and_ledger_metadata_conserved": True, "original_check_bindings_and_layout_conserved": True,
            "approval_required_before_writer_stop": True, "incomplete_expired_wrong_target_approval_denied": True,
            "preservation_sha256": request["preservation_sha256"]})


class NativeRestoreCapacityTests(unittest.TestCase):
    def test_disk_backed_restore_exceeds_former_768_mib_tmpfs_limit(self):
        artifact_root = Path(os.environ.get("PIPING_NATIVE_ARTIFACT_ROOT", "/data/artifacts/PWA-118-operator-carried-proof-20261014"))
        artifact_root.mkdir(parents=True, exist_ok=True)
        directory = Path(tempfile.mkdtemp(prefix="restore-capacity-", dir=artifact_root))
        directory.chmod(0o700)
        deployment = deploy.Deployment.__new__(deploy.Deployment)
        deployment.stage = directory
        deployment.plan = {"postgres_image": "postgres:18-alpine"}
        previous_diagnostics = deploy.COMMAND_DIAGNOSTICS
        deploy.COMMAND_DIAGNOSTICS = directory
        self.addCleanup(setattr, deploy, "COMMAND_DIAGNOSTICS", previous_diagnostics)
        with deployment.isolated_database("capacity-source") as database:
            with deployment.database_target(database):
                deployment.sql("CREATE TABLE capacity_payload(id integer PRIMARY KEY,payload text); "
                               "ALTER TABLE capacity_payload ALTER COLUMN payload SET STORAGE EXTERNAL; "
                               "INSERT INTO capacity_payload SELECT n,repeat(md5(n::text),256) "
                               "FROM generate_series(1,110000) n;")
                size = int(deployment.sql("SELECT pg_total_relation_size('capacity_payload');"))
                self.assertGreater(size, 768 * 1024 * 1024)
                expected = deployment.sql("SELECT count(*)::text||':'||sum(octet_length(payload))::text FROM capacity_payload;")
            dump = directory / "capacity.dump"
            with dump.open("xb") as stream:
                dump.chmod(0o600)
                deploy.run(database["compose"] + ["exec", "-T", "postgres", "pg_dump", "-Fc", "-U", "piping", "-d", "piping"],
                           output=stream, timeout=300)
        with deployment.isolated_database("capacity-restore") as database:
            with dump.open("rb") as stream, tempfile.TemporaryFile() as output:
                result = subprocess.run(database["compose"] + ["exec", "-T", "postgres", "pg_restore", "--exit-on-error",
                                        "-U", "piping", "-d", "piping"], stdin=stream, stdout=output, stderr=output,
                                        env=deploy.environment(), timeout=300, check=False)
            self.assertEqual(result.returncode, 0)
            with deployment.database_target(database):
                self.assertEqual(deployment.sql("SELECT count(*)::text||':'||sum(octet_length(payload))::text FROM capacity_payload;"), expected)
            deploy.write_json(directory / "capacity-receipt.json", {"source_relation_bytes": size,
                              "memory_limit_bytes": 768 * 1024 * 1024, "restored_rows": 110000,
                              "disk_backed": True, "dump_sha256": deploy.file_sha(dump)})


class NativeArchiveBootstrapTests(unittest.TestCase):
    """A genuine published image crosses into an empty, networkless Docker store."""

    def test_published_save_load_without_registry_or_repo_digest(self):
        artifact_root = Path(os.environ.get("PIPING_NATIVE_ARTIFACT_ROOT", "/data/artifacts/PWA-118-operator-carried-proof-20261014"))
        artifact_root.mkdir(parents=True, exist_ok=True)
        directory = Path(tempfile.mkdtemp(prefix="two-store-", dir=artifact_root))
        directory.chmod(0o700)
        published = PRODUCER["BACKEND_IMAGE"]
        previous_diagnostics = deploy.COMMAND_DIAGNOSTICS
        deploy.COMMAND_DIAGNOSTICS = directory
        self.addCleanup(setattr, deploy, "COMMAND_DIAGNOSTICS", previous_diagnostics)
        source = PRODUCER["APPLICATION_REVISION"]
        description = deploy.save_image(published, directory, "published-backend", None, "linux/amd64", source)
        # The engine's OCI save contains the original published manifest bytes;
        # no registry request or invented checksum supplies this authority.
        with tarfile.open(directory / description["archive"], "r:") as archive:
            raw = archive.extractfile("blobs/sha256/" + published.rsplit(":", 1)[1]).read()
        deploy.verify_manifest(raw, published, description["config_digest"])
        project = "pwa118-offline-" + uuid.uuid4().hex[:12]
        daemon_dir = directory / "client-store"
        daemon_dir.mkdir(mode=0o700)
        socket_root = deploy.checked_path(Path(os.environ.get("PIPING_NATIVE_SOCKET_ROOT", "/data/tmp")), directory=True)
        socket_dir = Path(tempfile.mkdtemp(prefix="pwa118-socket-", dir=socket_root))
        socket_dir.chmod(0o700)
        self.assertLess(len(os.fsencode(str(socket_dir / "docker.sock"))), 108)
        compose_file = directory / "daemon.compose.json"
        deploy.write_json(compose_file, {"services": {"engine": {
            "image": os.environ.get("PIPING_NATIVE_DIND_IMAGE", "docker:29.8.2-dind"),
            "privileged": True, "network_mode": "none", "cpus": 1, "mem_limit": "1g",
            "environment": {"DOCKER_TLS_CERTDIR": ""},
            "command": ["dockerd", "--host=unix:///socket/docker.sock", "--data-root=/data/store",
                        "--iptables=false", "--bridge=none", "--ip-masq=false"],
            "volumes": [{"type": "bind", "source": str(daemon_dir), "target": "/data"},
                        {"type": "bind", "source": str(socket_dir), "target": "/socket"}],
            "healthcheck": {"test": ["CMD", "docker", "-H", "unix:///socket/docker.sock", "info"],
                            "interval": "1s", "timeout": "3s", "retries": 60},
        }}})
        compose = ["docker", "compose", "-p", project, "-f", str(compose_file)]
        try:
            deploy.run(compose + ["up", "-d", "--pull", "never", "--no-build", "--wait", "--wait-timeout", "90"], timeout=120)
            # The enclosing operator-owned 0700 directory is the access boundary;
            # the image's docker GID need not match the workstation's docker GID.
            deploy.run(compose + ["exec", "-T", "engine", "chmod", "666", "/socket/docker.sock"])
            with patch.dict(os.environ, {"DOCKER_HOST": "unix://" + str(socket_dir / "docker.sock"),
                                         "DOCKER_TLS_VERIFY": "", "DOCKER_CONTEXT": ""}):
                self.assertEqual(deploy.run(["docker", "image", "ls", "-q"]).strip(), b"")
                with self.assertRaises(deploy.DeployError):
                    deploy.run(["docker", "pull", published], timeout=30)
                local = deploy.load_image(directory, description, "linux/amd64", source)
                self.assertEqual(deploy.exported_config(local, description["config_digest"], directory)["rootfs"]["type"], "layers")
                inspected = json.loads(deploy.run(["docker", "image", "inspect", local]))[0]
                self.assertFalse(inspected.get("RepoDigests"))
                deploy.write_json(directory / "native-bootstrap-receipt.json", {
                    "published_ref": published, "source": source, "local_id": local,
                    "archive_sha256": description["archive_sha256"], "registry_unavailable_before_load": True,
                    "stores": 2, "repo_digests": inspected.get("RepoDigests", [])})
        finally:
            deploy.run(compose + ["down", "--volumes", "--timeout", "30"], timeout=90)
            socket = socket_dir / "docker.sock"
            if socket.exists():
                socket.unlink()
            socket_dir.rmdir()


if __name__ == "__main__":
    unittest.main()
