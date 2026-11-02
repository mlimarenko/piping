"""Run only in a fresh, labelled disposable daemon with the release images loaded."""

import hashlib
import fcntl
import argparse
import json
import os
from pathlib import Path
import subprocess
import socket
import tempfile
import unittest

from scripts import deploy_release_bundle as deploy


ROOT = Path(__file__).resolve().parents[2]


class InstallLifecycleTests(unittest.TestCase):
    def command(self, *arguments, check=True, timeout=120, **kwargs):
        result = subprocess.run(arguments, capture_output=True, text=True,
                                env=self.environment, timeout=timeout, **kwargs)
        if check and result.returncode:
            # Installer/helper diagnostics may include private local state.
            (self.private / "last-failure.private.log").write_text(result.stdout + result.stderr)
            (self.private / "last-failure.private.log").chmod(0o600)
            self.fail(f"Native command failed with exit {result.returncode}; inspect private fixture output")
        return result

    def setUp(self):
        self.environment = dict(os.environ)
        info = json.loads(subprocess.run(["docker", "info", "--format", "{{json .Labels}}"],
                          capture_output=True, text=True, timeout=30, check=True).stdout)
        if "piping.fixture=install-lifecycle" not in info:
            raise RuntimeError("Installation lifecycle tests require their labelled disposable daemon")
        containers = subprocess.run(["docker", "ps", "-aq"], capture_output=True,
                                    text=True, timeout=30, check=True).stdout.splitlines()
        if len(containers) > 1:
            raise RuntimeError("Installation lifecycle fixture daemon is not fresh")
        self.private = Path(tempfile.mkdtemp(prefix="pwa164-install-", dir=os.environ["PIPING_NATIVE_ARTIFACT_ROOT"]))
        self.private.chmod(0o700)
        self.environment["HOME"] = str(self.private)
        self.environment.pop("COMPOSE_FILE", None)
        self.environment.pop("COMPOSE_PROJECT_NAME", None)
        self.install = self.private / "piping"

    def compose(self, *arguments):
        return self.command("docker", "compose", "--project-directory", str(self.install),
                            "--env-file", str(self.install / ".env"), "-f", str(self.install / "docker-compose.yml"),
                            "--profile", "edge", *arguments)

    def tearDown(self):
        if (self.install / ".env").exists():
            self.compose("down", "--volumes", "--timeout", "30")

    def sql(self, query):
        return self.compose("exec", "-T", "postgres", "psql", "-X", "-qAt", "-U", "piping", "-d", "piping",
                            "-v", "ON_ERROR_STOP=1", "-c", query).stdout.strip()

    def test_default_install_and_explicit_image_update_preserve_the_instance(self):
        # Reserve a real fixture-daemon port, then retry after releasing it.
        # Ambient application variables intentionally cannot alter saved config.
        with socket.socket() as occupied_port:
            occupied_port.bind(("0.0.0.0", 80))
            occupied_port.listen()
            failed_install = self.command("bash", str(ROOT / "install.sh"), check=False, timeout=300)
            self.assertNotEqual(failed_install.returncode, 0)
            self.assertTrue((self.install / ".fresh-install-in-progress").is_file())
            initial_settings = (self.install / ".env").read_bytes()
        # No script arguments or build: defaults are the actual pinned cohort.
        self.command("bash", str(ROOT / "install.sh"), timeout=300)
        self.assertTrue((self.install / ".env").read_bytes() == initial_settings,
                        "Retry replaced initial settings")
        self.assertFalse((self.install / ".fresh-install-in-progress").exists())
        # Preserve actual port-publication evidence for a failed interrupted boot.
        state = self.compose("ps", "--all", "--format", "json")
        (self.private / "initial-start-state.json").write_text(state.stdout)
        (self.private / "initial-start-state.json").chmod(0o600)
        with (self.install / ".deploy-release.lock").open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            refused_start = self.command("bash", str(ROOT / "install.sh"), check=False)
            self.assertNotEqual(refused_start.returncode, 0)
            self.assertIn("Another installation/update is active", refused_start.stderr)
        self.assertEqual(self.command("curl", "--silent", "--show-error", "--fail", "--cacert",
                         str(self.install / "docker/nginx/ssl/fullchain.pem"),
                         "https://localhost/api/v1/ready").returncode, 0)

        self.assertEqual(self.sql("SELECT max(version) FROM _sqlx_migrations WHERE success"), "24")
        fingerprint = self.sql("SELECT instance_fingerprint FROM license_instance_identity")
        self.sql("INSERT INTO settings_values(scope_type,key,value) VALUES "
                 "('instance','installer.fixture','{\"preserve\":true}'::jsonb)")
        env = self.install / ".env"
        with env.open("a") as stream:
            stream.write("\n# retained operator settings\nPIPING_OFFICIAL_CATALOG_REFRESH_ENABLED=false\nPIPING_LOG_LEVEL=warn\n")
        env_before = env.read_text()
        tls = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
               for p in (self.install / "docker/nginx/ssl").iterdir()}
        installed = json.loads(self.compose("config", "--format", "json").stdout)
        original = {role: installed["services"][role]["image"]
                    for role in ("backend", "frontend", "ansible-runner")}
        replacement = os.environ.get("PIPING_INSTALL_TEST_FRONTEND_IMAGE")
        if not replacement:
            raise RuntimeError("A genuine alternate published frontend image is required")
        # Empty or missing images fail before writers stop or assignments change.
        before_id = self.compose("ps", "-q", "backend").stdout.strip()
        failed = self.command("bash", str(ROOT / "install.sh"), "--skip-pull",
                              "--frontend-image", "piping-nonexistent:fixture", check=False)
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual(self.compose("ps", "-q", "backend").stdout.strip(), before_id)
        self.assertTrue(env.read_text() == env_before, "Refused update changed settings")
        # Real unapproved schema drift must fail the isolated restore proof.
        self.sql("CREATE TABLE public.installer_catalog_probe(id integer PRIMARY KEY)")
        self.sql("INSERT INTO public.installer_catalog_probe VALUES (1)")
        refused = self.command("bash", str(ROOT / "install.sh"), "--skip-pull",
                               "--frontend-image", replacement, check=False, timeout=1800)
        (self.private / "expected-refusal.private.log").write_text(refused.stdout + refused.stderr)
        (self.private / "expected-refusal.private.log").chmod(0o600)
        self.assertNotEqual(refused.returncode, 0)
        self.assertTrue(env.read_text() == env_before, "Unproved backup changed image assignments")
        running = self.compose("ps", "--status", "running", "--services").stdout.splitlines()
        self.assertIn("backend", running)
        self.assertIn("ansible-runner", running)
        self.assertEqual(self.compose("ps", "-q", "backend").stdout.strip(), before_id)
        differences = tuple((self.install / "backups").glob("release-*/readiness-difference.private.json"))
        self.assertEqual(len(differences), 1)
        self.assertTrue(any(item["name"] == "public.installer_catalog_probe"
                            for item in json.loads(differences[0].read_text())))
        self.assertEqual(self.sql("SELECT count(*) FROM public.installer_catalog_probe"), "1")
        self.sql("DROP TABLE public.installer_catalog_probe")
        # The operator removes the known test-only drift; writers never stopped.
        self.command("bash", str(ROOT / "install.sh"), "--skip-pull", "--frontend-image", replacement, timeout=1800)
        final = json.loads(self.compose("config", "--format", "json").stdout)
        requested_id = json.loads(self.command("docker", "image", "inspect", replacement).stdout)[0]["Id"]
        self.assertEqual(final["services"]["frontend"]["image"], requested_id)
        for role in ("backend", "ansible-runner"):
            expected = json.loads(self.command("docker", "image", "inspect", original[role]).stdout)[0]["Id"]
            self.assertEqual(final["services"][role]["image"], expected)
        self.assertTrue(self.sql("SELECT instance_fingerprint FROM license_instance_identity") == fingerprint,
                        "Instance identity changed")
        self.assertEqual(self.sql("SELECT value->>'preserve' FROM settings_values WHERE key='installer.fixture'"), "true")
        self.assertEqual(tls, {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                              for p in (self.install / "docker/nginx/ssl").iterdir()})
        def settings(text):
            return [line for line in text.splitlines()
                    if not line.startswith(("PIPING_BACKEND_IMAGE=", "PIPING_FRONTEND_IMAGE=", "PIPING_ANSIBLE_RUNNER_IMAGE="))]
        self.assertTrue(settings(env.read_text()) == settings(env_before), "Non-image settings changed")
        self.assertEqual(env.stat().st_mode & 0o777, 0o600)
        before_start = self.compose("ps", "-q").stdout.splitlines()
        self.command("bash", str(self.install / "install.sh"), timeout=300)
        self.assertEqual(self.compose("ps", "-q").stdout.splitlines(), before_start)
        self.assertEqual(self.command("curl", "--silent", "--show-error", "--fail", "--cacert",
                         str(self.install / "docker/nginx/ssl/fullchain.pem"),
                         "https://localhost/api/v1/ready").returncode, 0)
        # An interrupted transaction must not restart retained containers, even
        # after its process has exited and released the operation lock.
        stage = self.install / "backups/release-20261018T000000Z"
        stage.mkdir(mode=0o700)
        (stage / "journal").mkdir(mode=0o700)
        owner = deploy.Deployment(argparse.Namespace(root=self.install, stage=stage, project_name="fixture"))
        for phase in deploy.PHASES[:deploy.PHASES.index("migrating") + 1]:
            owner.advance(phase)
        self.compose("stop", "backend", "ansible-runner")
        refused = self.command("bash", str(self.install / "install.sh"), check=False)
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("Unresolved update journal", refused.stderr)
        self.assertFalse({"backend", "ansible-runner"}.intersection(
            self.compose("ps", "--status", "running", "--services").stdout.splitlines()))


if __name__ == "__main__":
    unittest.main()
