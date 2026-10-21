"""Verify image references with the Compose parser and installer argument parser."""

import json
import fcntl
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import uuid


ROOT = Path(__file__).resolve().parents[2]
IMAGE_REFS = {
    "PIPING_BACKEND_IMAGE": "registry.example:4567/piping/backend:qa",
    "PIPING_FRONTEND_IMAGE": "registry.example:4567/piping/frontend@sha256:" + "a" * 64,
    "PIPING_ANSIBLE_RUNNER_IMAGE": "registry.example:4567/piping/ansible-runner@sha256:" + "b" * 64,
}


class ComposeImageContractTests(unittest.TestCase):
    def test_fresh_compose_uses_saved_settings_not_ambient_application_variables(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "stack"
            environment = dict(os.environ, POSTGRES_PASSWORD="ambient-must-not-override",
                               PIPING_HTTP_PORT="18080", PIPING_BACKEND_IMAGE="ambient:must-not-override")
            script = ('source "$1"; INSTALL_DIR="$2"; PROJECT_NAME=fixture; '
                      'prepare_layout >/dev/null; run_compose docker compose --env-file "$INSTALL_DIR/.env" '
                      '--profile edge -f "$INSTALL_DIR/docker-compose.yml" config --format json')
            result = subprocess.run(["bash", "-c", script, "saved-settings", str(ROOT / "install.sh"), str(root)],
                                    env=environment, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, "Saved-settings Compose rendering failed")
            model = json.loads(result.stdout)
            saved = dict(line.split("=", 1) for line in (root / ".env").read_text().splitlines() if "=" in line)
            self.assertTrue(model["services"]["postgres"]["environment"]["POSTGRES_PASSWORD"] == saved["POSTGRES_PASSWORD"],
                            "Ambient credentials replaced the saved database credential")
            self.assertEqual(model["services"]["nginx"]["ports"][0]["published"], "80")
            self.assertEqual(model["services"]["backend"]["image"], saved["PIPING_BACKEND_IMAGE"])

    def test_fresh_install_refuses_existing_project_data_before_generating_settings(self):
        project = "pwa164-owned-" + uuid.uuid4().hex[:12]
        volume = project + "_postgres_data"
        subprocess.run(["docker", "volume", "create", "--label", "com.docker.compose.project=" + project, volume],
                       capture_output=True, check=True, timeout=30)
        try:
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory) / "stack"
                result = subprocess.run(["bash", str(ROOT / "install.sh"), "--install-dir", str(root),
                                         "--project-name", project], capture_output=True, text=True, timeout=30)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("restore its original .env", result.stderr)
                self.assertFalse((root / ".env").exists())
                self.assertFalse((root / "docker-compose.yml").exists())
                self.assertFalse((root / "docker/nginx/ssl/privkey.pem").exists())
        finally:
            subprocess.run(["docker", "volume", "rm", volume], capture_output=True, check=True, timeout=30)

    def test_installer_refuses_the_updaters_kernel_lock_before_starting_services(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".env").write_text("COMPOSE_PROJECT_NAME=fixture\nPIPING_PUBLIC_HOST=localhost\n")
            tls = root / "docker/nginx/ssl"
            tls.mkdir(parents=True)
            subprocess.run(["openssl", "req", "-x509", "-nodes", "-newkey", "rsa:2048", "-days", "1",
                            "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost",
                            "-keyout", str(tls / "privkey.pem"), "-out", str(tls / "fullchain.pem")],
                           check=True, capture_output=True, timeout=30)
            settings = (root / ".env").read_text()
            files = {path.relative_to(root) for path in root.rglob("*")}
            lock_path = root / ".deploy-release.lock"
            lock_path.touch(mode=0o600)
            with lock_path.open("r+") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                result = subprocess.run(["bash", str(ROOT / "install.sh"), "--install-dir", str(root)],
                                        capture_output=True, text=True, timeout=30)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Another installation/update is active", result.stderr)
            self.assertEqual((root / ".env").read_text(), settings)
            self.assertEqual({path.relative_to(root) for path in root.rglob("*")}, files | {Path(".deploy-release.lock")})

    def test_installer_refuses_symlink_ancestors_before_creating_the_stack(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "real").mkdir()
            (root / "link").symlink_to(root / "real", target_is_directory=True)
            result = subprocess.run(["bash", str(ROOT / "install.sh"), "--install-dir", str(root / "link" / "stack")],
                                    capture_output=True, text=True, timeout=30)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("must not contain symlinks", result.stderr)
            self.assertFalse((root / "real" / "stack").exists())

    def compose_model(self, *filenames: str) -> dict:
        environment = {key: value for key, value in os.environ.items() if not key.startswith("PIPING_")}
        environment.update(IMAGE_REFS)
        environment["PIPING_LICENSE_E2E_CLOUD_PROJECT"] = "piping-license-e2e-cloud-contract"
        environment["PIPING_IMAGE_TAG"] = "must-not-be-appended"
        result = subprocess.run(
            ["docker", "compose", "--env-file", "env/dev.env", "--profile", "proxy",
             *(argument for filename in filenames for argument in ("-f", filename)),
             "config", "--format", "json"],
            cwd=ROOT, env=environment, capture_output=True, text=True, timeout=30, check=True,
        )
        return json.loads(result.stdout)

    def compose_images(self, filename: str) -> dict[str, str]:
        return {name: service["image"] for name, service in self.compose_model(filename)["services"].items()}

    def test_main_compose_preserves_tag_and_digest_references(self) -> None:
        images = self.compose_images("docker-compose.yml")
        expected = {
            "backend": IMAGE_REFS["PIPING_BACKEND_IMAGE"],
            "proxy": IMAGE_REFS["PIPING_BACKEND_IMAGE"],
            "frontend": IMAGE_REFS["PIPING_FRONTEND_IMAGE"],
            "ansible-runner": IMAGE_REFS["PIPING_ANSIBLE_RUNNER_IMAGE"],
        }
        self.assertEqual({name: images[name] for name in expected}, expected)

    def test_licensing_fixture_reuses_canonical_services_with_isolated_data(self) -> None:
        main = self.compose_model("docker-compose.yml")
        fixture = self.compose_model("docker-compose.yml", "tests/compose/docker-compose.licensing.yml")
        self.assertEqual(fixture["services"]["backend"]["image"], IMAGE_REFS["PIPING_BACKEND_IMAGE"])
        for service in ("postgres", "redis", "backend"):
            self.assertEqual(fixture["services"][service]["image"], main["services"][service]["image"])
        self.assertEqual(fixture["services"]["backend"]["healthcheck"], main["services"]["backend"]["healthcheck"])
        self.assertEqual(fixture["services"]["postgres"]["environment"]["POSTGRES_DB"], "piping_license_e2e")
        self.assertTrue(fixture["services"]["backend"]["environment"]["PIPING_DATABASE__URL"].endswith("/piping_license_e2e"))
        for service in ("postgres", "redis"):
            self.assertEqual(fixture["services"][service].get("ports", []), [])
        self.assertEqual(fixture["services"]["backend"]["ports"][0]["host_ip"], "127.0.0.1")
        self.assertNotEqual(fixture["volumes"]["postgres_data"]["name"], main["volumes"]["postgres_data"]["name"])
        self.assertNotEqual(fixture["volumes"]["redis_data"]["name"], main["volumes"]["redis_data"]["name"])
        self.assertEqual(fixture["services"]["backend"]["environment"]["PIPING_LICENSE__BASE_URL"], "http://licensing-portal:8090")
        self.assertTrue(fixture["networks"]["licensing-e2e"]["external"])

    def test_cloud_licensing_overlay_replaces_a_fixed_control_plane_volume(self):
        with tempfile.TemporaryDirectory(prefix="cloud-compose-") as directory:
            base = Path(directory) / "compose.yml"
            base.write_text("""services:
  postgres:
    image: postgres:18-alpine
    volumes: [postgres-data:/var/lib/postgresql]
    ports: ['15432:5432']
  backend:
    image: fixture/backend
    ports: ['18090:8090']
    networks: [piping-space]
  billing-sim:
    image: fixture/billing
    ports: ['18181:8181']
volumes:
  postgres-data:
    name: piping-space-postgres-data
networks:
  piping-space:
""")
            project = "piping-license-e2e-cloud-guard"
            result = subprocess.run(
                ["docker", "compose", "-p", project, "-f", str(base), "-f",
                 str(ROOT / "tests/compose/docker-compose.licensing-cloud.yml"), "config", "--format", "json"],
                env=dict(os.environ, PIPING_LICENSE_E2E_CLOUD_PROJECT=project),
                capture_output=True, text=True, check=True, timeout=30,
            )
            model = json.loads(result.stdout)
            self.assertEqual(model["volumes"]["postgres-data"]["name"], project + "_postgres-data")
            self.assertEqual(model["volumes"]["postgres-data"]["labels"]["io.piping.fixture"], "licensing-e2e")
            self.assertEqual(model["networks"]["licensing-e2e"]["name"], project + "_licensing-e2e")
            self.assertIn("licensing-portal", model["services"]["backend"]["networks"]["licensing-e2e"]["aliases"])
            self.assertEqual(model["services"]["postgres"].get("ports", []), [])
            for service in ("backend", "billing-sim"):
                self.assertEqual(model["services"][service]["ports"][0]["host_ip"], "127.0.0.1")
            for region in ("EU", "RU"):
                self.assertEqual(model["services"]["backend"]["environment"][f"PIPING_SPACE_{region}_BILLING_PROVIDER"], "sim")

    def test_license_guard_refuses_a_foreign_volume_with_matching_container_labels(self):
        for owned in (True, False):
            with self.subTest(owned=owned):
                project = "piping-license-e2e-" + uuid.uuid4().hex[:12]
                volume = project + "_postgres_data"
                owner = project if owned else "unrelated-project"
                subprocess.run(["docker", "volume", "create", "--label", "com.docker.compose.project=" + owner,
                                "--label", "io.piping.fixture=licensing-e2e", volume],
                               check=True, capture_output=True, timeout=30)
                container = None
                try:
                    container = subprocess.run(
                        ["docker", "create", "--label", "com.docker.compose.project=" + project,
                         "--label", "com.docker.compose.service=postgres", "--label", "io.piping.fixture=licensing-e2e",
                         "--mount", "type=volume,source=" + volume + ",target=/var/lib/postgresql", "postgres:18-alpine"],
                        check=True, capture_output=True, text=True, timeout=60,
                    ).stdout.strip()
                    result = subprocess.run(
                        ["bash", "-c", 'set -eu; source "$1"; LICENSE_CONTAINER="$2"; license_fixture_database_mount "$3" postgres_data',
                         "volume-guard", str(ROOT / "scripts/license-e2e-fixture.sh"), container, project],
                        capture_output=True, text=True, timeout=30,
                    )
                    self.assertEqual(result.returncode == 0, owned)
                    if not owned:
                        self.assertIn("not owned by this licensing fixture", result.stderr)
                finally:
                    if container:
                        subprocess.run(["docker", "rm", container], check=True, capture_output=True, timeout=30)
                    subprocess.run(["docker", "volume", "rm", volume], check=True, capture_output=True, timeout=30)

    def test_licensing_fixture_cross_project_transport_and_outage_recovery(self):
        """Probe the canonical backend network, independently of license signing."""
        nonce = uuid.uuid4().hex[:12]
        app_project = "piping-license-e2e-app-" + nonce
        cloud_project = "piping-license-e2e-cloud-" + nonce
        env = dict(os.environ, **IMAGE_REFS, PIPING_LICENSE_E2E_CLOUD_PROJECT=cloud_project,
                   PIPING_LICENSE_E2E_PORTAL_PORT="0")
        with tempfile.TemporaryDirectory(prefix="licensing-transport-") as directory:
            root = Path(directory)
            cloud = root / "cloud.json"
            cloud.write_text(json.dumps({
                "services": {
                    "backend": {"image": "nginx:1.30-alpine", "networks": ["piping-space"],
                                "command": ["sh", "-ec", "printf '%s' 'server { listen 8090; location /readyz { return 200 fixture-ready; } }' > /etc/nginx/conf.d/default.conf; exec nginx -g 'daemon off;'"],
                                "healthcheck": {"test": ["CMD-SHELL", "wget -qO- http://127.0.0.1:8090/readyz >/dev/null"],
                                                "interval": "1s", "timeout": "1s", "retries": 20}},
                    "postgres": {"image": "postgres:18-alpine", "volumes": ["postgres-data:/var/lib/postgresql"]},
                    "billing-sim": {"image": "nginx:1.30-alpine"},
                },
                "volumes": {"postgres-data": {"name": "piping-space-postgres-data"}},
                "networks": {"piping-space": {}},
            }))
            probe = root / "probe.json"
            # CI uses a small tool image; target acceptance can supply the actual
            # published backend image without changing the service's networks.
            probe.write_text(json.dumps({"services": {"backend": {
                "image": os.environ.get("PIPING_LICENSE_E2E_BACKEND_PROBE_IMAGE", "postgres:18-alpine")}}}))
            def command(project, files):
                return ["docker", "compose", "--project-directory", str(ROOT), "--env-file", str(ROOT / "env/dev.env"),
                        "-p", project, *(part for file in files for part in ("-f", str(file)))]
            cloud_cmd = command(cloud_project, [cloud, ROOT / "tests/compose/docker-compose.licensing-cloud.yml"])
            app_cmd = command(app_project, [ROOT / "docker-compose.yml", ROOT / "tests/compose/docker-compose.licensing.yml", probe])
            volumes = []
            for prefix, project in ((cloud_cmd, cloud_project), (app_cmd, app_project)):
                model = json.loads(subprocess.run(prefix + ["config", "--format", "json"], env=env,
                                                 check=True, capture_output=True, text=True, timeout=30).stdout)
                self.assertTrue(all(item["name"].startswith(project + "_") for item in model["volumes"].values()))
                volumes.extend(item["name"] for item in model["volumes"].values())
            request = 'if command -v curl >/dev/null; then exec curl --connect-timeout 2 --max-time 4 -fsS "$PIPING_LICENSE__BASE_URL/readyz"; else exec wget -q -T 4 -O- "$PIPING_LICENSE__BASE_URL/readyz"; fi'
            def fetch():
                return subprocess.run(app_cmd + ["run", "--rm", "--no-deps", "--entrypoint", "sh", "backend", "-ec", request],
                                      env=env, capture_output=True, text=True, timeout=30)
            try:
                subprocess.run(cloud_cmd + ["up", "-d", "--no-deps", "--wait", "--wait-timeout", "30", "backend"],
                               env=env, check=True, capture_output=True, timeout=60)
                healthy = fetch()
                self.assertEqual(healthy.returncode, 0, "App network cannot reach its configured licensing endpoint")
                self.assertEqual(healthy.stdout.strip(), "fixture-ready")
                subprocess.run(cloud_cmd + ["stop", "backend"], env=env, check=True, capture_output=True, timeout=30)
                self.assertNotEqual(fetch().returncode, 0, "stopped Cloud fixture remained reachable")
                subprocess.run(cloud_cmd + ["up", "-d", "--no-deps", "--wait", "--wait-timeout", "30", "backend"],
                               env=env, check=True, capture_output=True, timeout=60)
                recovered = fetch()
                self.assertEqual(recovered.returncode, 0)
                self.assertEqual(recovered.stdout.strip(), "fixture-ready")
            finally:
                for prefix in (app_cmd, cloud_cmd):
                    subprocess.run(prefix + ["down", "--remove-orphans"], env=env, capture_output=True, timeout=30)
                # Only fresh nonce-scoped names from the validated native model.
                for volume in volumes:
                    subprocess.run(["docker", "volume", "rm", volume], capture_output=True, timeout=30)

    def test_installer_accepts_each_complete_reference(self) -> None:
        result = subprocess.run(
            ["bash", "-c", 'source "$1"; shift; parse_args "$@"; '
             'printf "%s\\n" "$BACKEND_IMAGE" "$FRONTEND_IMAGE" "$ANSIBLE_RUNNER_IMAGE"',
             "image-contract", str(ROOT / "install.sh"),
             "--backend-image", IMAGE_REFS["PIPING_BACKEND_IMAGE"],
             "--frontend-image", IMAGE_REFS["PIPING_FRONTEND_IMAGE"],
             "--ansible-runner-image", IMAGE_REFS["PIPING_ANSIBLE_RUNNER_IMAGE"]],
            capture_output=True, text=True, timeout=5, check=True,
        )
        self.assertEqual(result.stdout.splitlines(), list(IMAGE_REFS.values()))

    def test_installer_rejects_env_injection_and_invalid_digest(self) -> None:
        for reference in ("", "backend:tag\nPIPING_AUTH_LOCAL_ENABLED=false",
                          "${PIPING_BACKEND_IMAGE}", "https://registry.example/backend",
                          "backend@sha256:abc"):
            with self.subTest(reference=reference):
                result = subprocess.run(
                    ["bash", "-c", 'source "$1"; validate_image_reference "$2"',
                     "image-contract", str(ROOT / "install.sh"), reference],
                    capture_output=True, text=True, timeout=5,
                )
                self.assertNotEqual(result.returncode, 0)

    def test_installer_help_runs_from_stdin(self) -> None:
        result = subprocess.run(
            ["bash", "-s", "--", "--help"],
            input=(ROOT / "install.sh").read_text(encoding="utf-8"),
            capture_output=True, text=True, timeout=5, check=True,
        )
        self.assertIn("--ansible-runner-image", result.stdout)

    def test_installer_bare_arguments_do_not_request_an_image_update(self) -> None:
        result = subprocess.run(
            ["bash", "-c", 'source "$1"; parse_args; printf "%s\\n" "${#UPDATE_IMAGES[@]}"',
             "image-contract", str(ROOT / "install.sh")],
            capture_output=True, text=True, timeout=5, check=True,
        )
        self.assertEqual(result.stdout.strip(), "0")

    def test_installer_serializes_explicit_image_references(self) -> None:
        with tempfile.TemporaryDirectory(prefix="piping-image-contract-") as directory:
            result = subprocess.run(
                ["bash", "-c", 'source "$1"; shift; parse_args "$@"; '
                 'for path in "${STACK_SOURCE_PATHS[@]}"; do '
                 'fixture="${SOURCE_BASE_URL#file://}/$path"; '
                 'mkdir -p "$(dirname "$fixture")"; : > "$fixture"; done; '
                 'prepare_layout >/dev/null; source "$INSTALL_DIR/.env"; '
                 'printf "%s\\n" "$PIPING_BACKEND_IMAGE" "$PIPING_FRONTEND_IMAGE" '
                 '"$PIPING_ANSIBLE_RUNNER_IMAGE"',
                  "image-contract", str(ROOT / "install.sh"),
                 "--install-dir", directory,
                 "--source-base-url", f"file://{directory}/source",
                 "--backend-image", IMAGE_REFS["PIPING_BACKEND_IMAGE"],
                 "--frontend-image", IMAGE_REFS["PIPING_FRONTEND_IMAGE"],
                 "--ansible-runner-image", IMAGE_REFS["PIPING_ANSIBLE_RUNNER_IMAGE"]],
                input="y\n", capture_output=True, text=True, timeout=5, check=True,
            )
            self.assertEqual(result.stdout.splitlines()[-3:], list(IMAGE_REFS.values()))


if __name__ == "__main__":
    unittest.main()
