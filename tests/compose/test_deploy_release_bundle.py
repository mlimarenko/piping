"""Exercise archive-backed deployment against isolated native Docker resources.

No Docker-unavailable skip: this discovery route requires a working engine.
Every resource has a per-run project name; cleanup never targets the host stack.
scripts/ci-compose-native.py places this client in the daemon's network namespace.
"""

from __future__ import annotations

import copy
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
                           "postgres_image": json.loads(deploy.run(["docker", "image", "inspect", "postgres:18-trixie"]))[0]["Id"]}
        with deployment.isolated_database("native") as database:
            container = deploy.run(database["compose"] + ["ps", "-q", "postgres"]).decode().strip()
            deploy.run(["docker", "exec", container, "mkdir", "-p", "/proof/config"])
            for source, target in ((binary, "/proof/piping-backend"), (migrations, "/proof/migrations"),
                                   (ROOT / "apps/api/config/default.toml", "/proof/config/default.toml")):
                deploy.run(["docker", "cp", str(source), container + ":" + target], timeout=300)

            def migrate(*, refused=False):
                url = database["url"].replace("@postgres:", "@127.0.0.1:")
                command = ["docker", "exec", "-w", "/proof", "-e", "PIPING_START_MODE=migrate",
                           "-e", "PIPING_DATABASE__URL=" + url, "-e", "PIPING_MIGRATIONS_DIR=/proof/migrations",
                           "-e", "PIPING_REDIS__URL=redis://127.0.0.1:1",
                           "-e", "PIPING_SECRETS__CIPHER_KEY_B64=" + SYNTHETIC_CIPHER_KEY,
                           "-e", "PIPING_AUTH__PASSWORD_PEPPER=" + uuid.uuid4().hex,
                           "-e", "PIPING_AUTH__LOCAL_ENABLED=false",
                           "-e", "PIPING_OBSERVABILITY__OTEL_ENABLED=false", container, "/proof/piping-backend"]
                with (root / ("rejected.log" if refused else "migrate.log")).open("ab") as output:
                    result = subprocess.run(command, stdout=output, stderr=output, timeout=300,
                                            env=deploy.environment(), check=False)
                self.assertEqual(result.returncode != 0, refused, "See private native migration logs")

            migrate()
            with deployment.database_target(database):
                self.assertEqual(len(deployment.guard_history()), 1)
                catalog = deployment.catalog()
                self.assertTrue(any(o["kind"] == "collation" and o["name"] == "public.natural_sort" for o in catalog))
                self.assertTrue(any(o["kind"] == "trigger" and o["definition"]["internal"] for o in catalog))
                deployment.sql("INSERT INTO secrets_variable(id,key) VALUES "
                               "('00000000-0000-4000-8000-000000000139','native-preservation');"
                               "INSERT INTO secrets_store(secret_id,encrypted_value) VALUES "
                               "('00000000-0000-4000-8000-000000000139',encode(gen_random_bytes(64),'hex'));"
                               "UPDATE instance_branding SET app_name='Customized installation';")
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
            raise AssertionError(f"{Path(args[0]).name} {args[1:3]} failed ({result.returncode})")
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
        cls.addClassCleanup(cls.cleanup)
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
        native_binary = Path(os.environ["PIPING_NATIVE_BACKEND_BINARY"])
        if deploy.file_sha(native_binary) != os.environ["PIPING_NATIVE_BACKEND_SHA256"]:
            raise AssertionError("native fixture executable identity mismatch")
        shutil.copy2(native_binary, context / "piping-backend")
        (context / "config").mkdir()
        shutil.copyfile(ROOT / "apps/api/config/default.toml", context / "config/default.toml")
        (context / "entrypoint.sh").write_text(
            '#!/bin/sh\nif [ "${PIPING_START_MODE:-}" = migrate ]; then exec /usr/local/bin/piping-backend; fi\nexec tail -f /dev/null\n'
        )
        (context / "Dockerfile").write_text(
            "FROM postgres:18-trixie\nARG REVISION\n"
            "LABEL org.opencontainers.image.revision=$REVISION\n"
            "COPY migrations /app/migrations\nCOPY entrypoint.sh /entrypoint.sh\n"
            "COPY piping-backend /usr/local/bin/piping-backend\nCOPY config /app/config\nWORKDIR /app\n"
            'RUN chmod 755 /entrypoint.sh\nENTRYPOINT ["/entrypoint.sh"]\n'
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
                            # Disposable private-network SQL fixture, no credentials.
                            "POSTGRES_HOST_AUTH_METHOD": "trust",
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
                    "backend": {
                        "image": "${PIPING_BACKEND_IMAGE}",
                        "cpus": 1, "mem_limit": "1g",
                        "build": {"context": "/not-an-owned-build-context"},
                        "environment": {"PIPING_DATABASE__URL": "postgresql://piping@postgres:5432/piping",
                                        "PIPING_REDIS__URL": "redis://127.0.0.1:1",
                                        "PIPING_SECRETS__CIPHER_KEY_B64": SYNTHETIC_CIPHER_KEY,
                                        "PIPING_AUTH__PASSWORD_PEPPER": uuid.uuid4().hex,
                                        "PIPING_AUTH__LOCAL_ENABLED": "false",
                                        "PIPING_OBSERVABILITY__OTEL_ENABLED": "false"},
                    },
                    "ansible-runner": {"image": "${PIPING_ANSIBLE_RUNNER_IMAGE}", "cpus": 0.5, "mem_limit": "128m",
                        "volumes": [{"type": "bind", "source": str(cls.start_dependency),
                                     "target": "/writer-start-dependency", "read_only": True,
                                     "bind": {"create_host_path": False}}]},
                    "frontend": {"image": "${PIPING_FRONTEND_IMAGE}", "cpus": 0.5, "mem_limit": "128m"},
                    "proxy": {"image": "${PIPING_BACKEND_IMAGE}", "profiles": ["edge"], "cpus": 0.5, "mem_limit": "128m"},
                }
            },
            indent=2,
        ).encode()
        (cls.root / "docker-compose.yml").write_bytes(cls.compose_bytes)
        cls.compose("up", "-d", "--wait", "--wait-timeout", "180")
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
    def sql(cls, statement: str) -> str:
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
        ).stdout.strip()

    @classmethod
    def seed_history(cls):
        cls.command("docker", "run", "--rm", "--pull", "never", "--network", cls.project + "_default",
                    "-e", "PIPING_START_MODE=migrate", "-e", "PIPING_REDIS__URL=redis://127.0.0.1:1",
                    "-e", "PIPING_SECRETS__CIPHER_KEY_B64=" + SYNTHETIC_CIPHER_KEY,
                    "-e", "PIPING_AUTH__PASSWORD_PEPPER=" + uuid.uuid4().hex,
                    "-e", "PIPING_AUTH__LOCAL_ENABLED=false",
                    "-e", "PIPING_DATABASE__URL=postgresql://piping@postgres:5432/piping", cls.prefix_image)
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
    def test_native_fixture_registry_postgres_packaged_source(self):
        self.writers()
        self.assertEqual(
            self.sql("SELECT count(*) FROM _sqlx_migrations WHERE success;"),
            "1",
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
            self.assertEqual(result.returncode, 0, result.stderr)
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
            self.assertEqual(self.sql("SELECT count(*) FROM _sqlx_migrations WHERE success;"), "1")
            self.assertEqual(self.sql("SELECT NOT enabled FROM playbook_schedule WHERE id='" + SCHEDULE + "';"), "t")
            self.assertTrue((stage / "consumed-receipt.json").exists())
            result = self.deploy("apply", stage, extra=extra)
            self.assertEqual(result.returncode, 2)
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
            self.assertEqual(self.sql("SELECT count(*) FROM _sqlx_migrations WHERE success;"), "1")
            deploy.write_json(self.root / "native-flow-receipt.json", {
                "schema": "piping.compose-native-flow/v1", "helper_sha256": deploy.helper_identity(),
                 "synthetic_transport_cohort": True, "native_migrator_proved": True,
                 "application_source": self.application_commit,
                "candidate_absent_before_stage": True, "registry_unavailable_before_stage": True,
                "restore_failure_rejected": True, "receipt_forgery_rejected": True, "replay_rejected": True,
                 "configuration_database_trigger_drift_rejected": True, "env_only_baseline": True,
                "optional_proxy_absent": True, "failed_start_writers_stopped": True,
                  "ledger_versions": 1})
            # Explicit fixture recovery uses the current successful candidate,
            # never an old prefix binary or an automatic helper rollback.
            self.start_dependency.write_bytes(b"owned disposable writer mount")
            self.start_dependency.chmod(0o600)
            self.compose("up", "-d", "--no-deps", "--pull", "never", "--no-build", "backend", "ansible-runner")
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
        owner.sql = lambda query: self.sql(query).encode()
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
        metadata.sql = lambda query: self.sql(query).encode()
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
                       deploy.expected_catalog(self.canonical_catalog, plan["contract"])}
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
