"""Unsafe fixture selections must fail before Docker or an HTTP mutation."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "scripts/license-e2e-fixture.sh"


class LicenseFixtureTests(unittest.TestCase):
    def refuse(self, function, changes, message):
        with tempfile.TemporaryDirectory(prefix="license-target-") as directory:
            temporary = Path(directory)
            calls = temporary / "docker-calls"
            spy = temporary / "docker"
            spy.write_text('#!/bin/sh\nprintf unexpected > "$FIXTURE_CALLS"\nexit 91\n')
            spy.chmod(0o755)
            env = {key: value for key, value in os.environ.items()
                   if not key.startswith(("PIPING_", "LICENSE_", "BACKEND_URL", "COMPOSE_"))}
            env.update(PATH=str(temporary) + os.pathsep + env["PATH"], FIXTURE_CALLS=str(calls))
            env.update(changes)
            result = subprocess.run(
                ["bash", "-c", 'set -eu; source "$1"; "$2" "$3"',
                 "fixture-refusal", str(FIXTURE), function, str(ROOT)],
                env=env, capture_output=True, text=True, timeout=10,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(message, result.stderr)
            self.assertFalse(calls.exists(), "unsafe target reached Docker")

    def test_ordinary_installation_project_is_not_a_fixture(self):
        for project in ("piping", "production", "../piping-license-e2e", "piping-license-e2e;true"):
            with self.subTest(project=project):
                self.refuse("license_fixture_app", {"PIPING_LICENSE_E2E_PROJECT": project},
                            "project must be an isolated")

    def test_arbitrary_database_and_container_overrides_are_refused(self):
        for key in ("LICENSE_POSTGRES_CONTAINER", "LICENSE_DATABASE_NAME"):
            with self.subTest(key=key):
                self.refuse("license_fixture_app", {key: "existing-installation"}, "overrides are not accepted")

    def test_nonfixture_backend_urls_and_invalid_ports_are_refused(self):
        self.refuse("license_fixture_app", {"BACKEND_URL": "https://app.example.invalid"}, "loopback licensing fixture")
        for port in ("0", "65536", "-1", "80;true"):
            with self.subTest(port=port):
                self.refuse("license_fixture_app", {"PIPING_LICENSE_E2E_PORT": port}, "valid decimal port")

    def test_outage_requires_an_explicit_separate_cloud_project(self):
        self.refuse("license_fixture_cloud", {}, "explicit isolated Cloud project")
        self.refuse("license_fixture_cloud", {"PIPING_LICENSE_E2E_CLOUD_PROJECT": "piping-license-e2e"},
                    "explicit isolated Cloud project")
        self.refuse("license_fixture_cloud", {"PIPING_LICENSE_E2E_PROJECT": "piping-license-e2e-qa",
                    "PIPING_LICENSE_E2E_CLOUD_PROJECT": "piping-license-e2e-qa"}, "explicit isolated Cloud project")
        self.refuse("license_fixture_app", {"PIPING_LICENSE_E2E_PROJECT": "piping-license-e2e-qa",
                    "PIPING_LICENSE_E2E_CLOUD_PROJECT": "piping-license-e2e-qa"}, "separate fixture projects")

    def test_outage_refuses_implicit_paths_and_unverified_portal_targets(self):
        base = {"PIPING_LICENSE_E2E_CLOUD_PROJECT": "piping-license-e2e-cloud"}
        self.refuse("license_fixture_cloud", dict(base, PIPING_LICENSE_E2E_CLOUD_COMPOSE="../public/docker-compose.yml"),
                    "absolute Cloud Compose file")
        base["PIPING_LICENSE_E2E_CLOUD_COMPOSE"] = str(ROOT / "docker-compose.yml")
        self.refuse("license_fixture_cloud", dict(base, PIPING_SPACE_URL="https://portal.example.invalid"),
                    "loopback licensing fixture")
        self.refuse("license_fixture_cloud", dict(base, COMPOSE_SERVICE="production"),
                    "unverified portal target overrides")
