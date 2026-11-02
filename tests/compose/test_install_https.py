"""Native installer/stock-nginx regression tests; requires Docker and host OpenSSL.

Run: python3 -m unittest tests.compose.test_install_https -v
"""

import hashlib
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest
import uuid

from scripts import deploy_release_bundle as deploy


ROOT = Path(__file__).resolve().parents[2]


def run(*args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, timeout=60, **kwargs)


class InstallHttpsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.network = "pwa163-" + uuid.uuid4().hex[:12]
        result = run("docker", "network", "create", "--opt",
                     "com.docker.network.bridge.host_binding_ipv4=127.0.0.1", cls.network)
        if result.returncode:
            raise RuntimeError("Docker network creation failed")

    @classmethod
    def tearDownClass(cls):
        run("docker", "network", "rm", cls.network)

    def setUp(self):
        scratch = Path(os.environ.get("PIPING_TEST_ARTIFACTS", "/data/tmp"))
        scratch.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="pwa163-", dir=scratch)
        self.install = Path(self.temp.name)
        self.containers = []
        result = self.prepare()
        self.assertEqual(result.returncode, 0, "Installer layout/TLS preparation failed")

    def tearDown(self):
        for container in self.containers:
            run("docker", "rm", "-f", container)
        self.temp.cleanup()

    def prepare(self, host="127.0.0.1", https_port="443"):
        # Source the actual installer owner; no fake Docker or source parser.
        script = f"""
source {shlex.quote(str(ROOT / 'install.sh'))}
INSTALL_DIR={shlex.quote(str(self.install))}
SOURCE_BASE_URL={shlex.quote(ROOT.as_uri())}
PROJECT_NAME=pwa163-test
parse_args --hostname {shlex.quote(host)} --http-port 18944 --https-port {shlex.quote(https_port)}
validate_public_host
validate_port "$HTTPS_PORT"
prepare_layout
"""
        return run("bash", "-c", script)

    def compose(self):
        result = run("docker", "compose", "--env-file", str(self.install / ".env"),
                     "--profile", "edge", "-f", str(self.install / "docker-compose.yml"), "config", "--format", "json")
        self.assertEqual(result.returncode, 0, "Native Compose validation failed")
        return json.loads(result.stdout)["services"]["nginx"]

    def start(self, port="443", host="127.0.0.1", real_backend=False):
        self.install = Path(self.temp.name) / ("stack-" + uuid.uuid4().hex[:8])
        self.install.mkdir()
        self.assertEqual(self.prepare(host, port).returncode, 0, "Installer configuration failed")
        service = self.compose()
        name = "pwa163-" + uuid.uuid4().hex[:12]
        self.containers.append(name)
        fixture = {
            "services": {"nginx": {
                "image": service["image"], "container_name": name,
                "mem_limit": "128m", "cpus": 0.5,
                "extra_hosts": ["frontend:127.0.0.1"] + ([] if real_backend else ["backend:127.0.0.1"]),
                "ports": ["127.0.0.1::80", "127.0.0.1::443"],
                "environment": service["environment"],
                "volumes": service["volumes"],
                "healthcheck": {**service["healthcheck"], "interval": "1s", "start_period": "1s"},
                "networks": ["test"],
            }},
            "networks": {"test": {"external": True, "name": self.network}},
        }
        path = self.install / (name + ".json")
        path.write_text(json.dumps(fixture))
        self.proxy_fixture = path
        # Native Compose owns the bounded TLS health wait.
        result = run("docker", "compose", "-p", name, "-f", str(path),
                     "up", "-d", "--wait", "--wait-timeout", "15")
        self.assertEqual(result.returncode, 0, "Stock nginx never served TLS health")
        inspection = json.loads(run("docker", "inspect", name).stdout)[0]
        ports = inspection["NetworkSettings"]["Ports"]
        return name, ports["80/tcp"][0]["HostPort"], ports["443/tcp"][0]["HostPort"]

    def test_edge_refresh_resolves_replaced_backend_before_accepting_update(self):
        # A real HTTP origin isolates Docker DNS/proxy behaviour from database
        # migrations. The lifecycle test separately exercises the actual backend.
        origin_config = Path(self.temp.name) / "origin.conf"
        origin_config.write_text('events {} http { server { listen 7000; '
                                 'location /api/v1/ready { return 200 "ready\\n"; } } }')
        origin = "pwa163-origin-" + uuid.uuid4().hex[:12]
        self.containers.append(origin)
        origin_fixture = Path(self.temp.name) / "origin.json"
        origin_fixture.write_text(json.dumps({
            "services": {"backend": {
                "image": "nginx:1.30-alpine", "container_name": origin,
                "volumes": [str(origin_config) + ":/etc/nginx/nginx.conf:ro"],
                "healthcheck": {"test": ["CMD", "curl", "-fsS", "http://127.0.0.1:7000/api/v1/ready"],
                                "interval": "1s", "timeout": "2s", "retries": 10},
                "networks": {"test": {"aliases": ["backend"]}},
            }}, "networks": {"test": {"external": True, "name": self.network}},
        }))
        origin_compose = ["docker", "compose", "-p", origin, "-f", str(origin_fixture)]
        self.assertEqual(run(*origin_compose, "up", "-d", "--wait", "--wait-timeout", "15").returncode, 0)
        proxy, _, https = self.start(real_backend=True)
        def ready():
            return run("curl", "--silent", "--show-error", "--cacert",
                       str(self.install / "docker/nginx/ssl/fullchain.pem"), "--max-time", "4",
                       "--output", "/dev/null", "--write-out", "%{http_code}",
                       f"https://127.0.0.1:{https}/api/v1/ready")
        self.assertEqual((ready().returncode, ready().stdout), (0, "200"))
        def address():
            return json.loads(run("docker", "inspect", origin).stdout)[0]["NetworkSettings"]["Networks"][self.network]["IPAddress"]
        previous_address = address()
        self.assertEqual(run("docker", "rm", "-f", origin).returncode, 0)
        # Reserve the released address so replacement cannot accidentally reuse
        # it and conceal the stale upstream address cached by the running edge.
        holder = "pwa163-holder-" + uuid.uuid4().hex[:12]
        self.containers.append(holder)
        self.assertEqual(run("docker", "run", "-d", "--name", holder, "--network", self.network,
                             "--ip", previous_address, "--entrypoint", "tail", "nginx:1.30-alpine",
                             "-f", "/dev/null").returncode, 0)
        self.assertEqual(run(*origin_compose, "up", "-d", "--wait", "--wait-timeout", "15").returncode, 0)
        self.assertNotEqual(address(), previous_address)
        self.assertEqual(run("docker", "exec", origin, "curl", "-fsS",
                             "http://127.0.0.1:7000/api/v1/ready").returncode, 0)
        self.assertEqual(ready().stdout, "502")
        self.assertEqual(run("docker", "exec", proxy, "curl", "-fsS", "--insecure",
                             "https://127.0.0.1/health").returncode, 0)
        # Exercise the update owner's real edge refresh, retaining the prepared
        # settings and certificate while replacing the stale proxy process.
        (self.install / "docker-compose.yml").write_bytes(self.proxy_fixture.read_bytes())
        owner = deploy.Deployment(argparse.Namespace(root=self.install, project_name=proxy,
                                 stage=self.install / "backups/release-20261020T000000Z"))
        owner.plan = {"image_file": ".env"}
        owner.refresh_edge_upstreams()
        published = json.loads(run("docker", "inspect", proxy).stdout)[0]["NetworkSettings"]["Ports"]["443/tcp"][0]["HostPort"]
        https = published
        self.assertEqual((ready().returncode, ready().stdout), (0, "200"))

    def test_tls_and_env_preservation(self):
        before = {p.name: hashlib.sha256(p.read_bytes()).digest()
                  for p in (self.install / "docker/nginx/ssl").iterdir()}
        env_before = (self.install / ".env").read_bytes()
        self.assertEqual(self.prepare().returncode, 0)
        self.assertTrue(env_before == (self.install / ".env").read_bytes(), ".env changed on repeat install")
        self.assertEqual((self.install / ".env").stat().st_mode & 0o777, 0o600)
        self.assertEqual(before, {p.name: hashlib.sha256(p.read_bytes()).digest()
                                 for p in (self.install / "docker/nginx/ssl").iterdir()})
        self.assertEqual((self.install / "docker/nginx/ssl/privkey.pem").stat().st_mode & 0o777, 0o600)

    def test_compose_environment_drives_redirect(self):
        for external_port, suffix in (("18945", ":18945"), ("443", "")):
            with self.subTest(external_port=external_port):
                name, http, https = self.start(external_port)
                result = run("curl", "-fsk", "--max-time", "5", f"https://127.0.0.1:{https}/health")
                self.assertEqual((result.returncode, result.stdout), (0, "ok\n"))
                result = run("curl", "-sS", "-I", "--max-time", "5", "-H", "Host: attacker.example",
                             f"http://127.0.0.1:{http}/path?q=1")
                self.assertEqual(result.returncode, 0)
                self.assertIn(f"location: https://127.0.0.1{suffix}/path?q=1", result.stdout.lower().splitlines())
                self.assertIn("308", result.stdout.splitlines()[0])
                self.assertEqual(run("docker", "exec", name, "sh", "-c",
                                     self.compose()["healthcheck"]["test"][1]).returncode, 0)

    def test_partial_and_invalid_operator_material_is_not_replaced(self):
        key = self.install / "docker/nginx/ssl/privkey.pem"
        before = key.read_bytes()
        cert = self.install / "docker/nginx/ssl/fullchain.pem"
        cert.unlink()
        self.assertNotEqual(self.prepare().returncode, 0)
        self.assertTrue(key.read_bytes() == before, "Existing key changed")
        self.assertFalse(cert.exists())
        cert.write_text("invalid certificate\n")
        self.assertNotEqual(self.prepare().returncode, 0)
        self.assertTrue(key.read_bytes() == before, "Existing key changed")
        self.assertEqual(cert.read_text(), "invalid certificate\n")

    def test_untrusted_inputs_are_rejected(self):
        for host in ("evil.example/path", "evil.example;return 200", "a..b", "-bad", "x:443", "x\ny"):
            self.assertNotEqual(self.prepare(host).returncode, 0)
        for port in ("0", "65536", "0443", "443;return 200", "9" * 100):
            script = f"source {shlex.quote(str(ROOT / 'install.sh'))}; validate_port {shlex.quote(port)}"
            self.assertNotEqual(run("bash", "-c", script).returncode, 0)

    def test_certificate_identity_matches_public_host_without_rotation(self):
        cases = (
            ("generated-dns", "app.example.com", "app.example.com", None, 0),
            ("generated-ipv4", "127.0.0.1", "127.0.0.1", None, 0),
            ("supplied-dns", "app.example.com", "app.example.com", "DNS:app.example.com", 0),
            ("supplied-ipv4", "127.0.0.1", "127.0.0.1", "IP:127.0.0.1", 0),
            ("supplied-wildcard", "app.example.com", "app.example.com", "DNS:*.example.com", 0),
            ("wrong-dns-san", "app.example.com", "app.example.com", "DNS:other.example.com", 1),
            ("wrong-ipv4-san", "127.0.0.1", "127.0.0.1", "IP:127.0.0.2", 1),
            ("wildcard-depth", "sub.app.example.com", "sub.app.example.com", "DNS:*.example.com", 1),
            ("generated-hostname-change", "localhost", "app.example.com", None, 1),
        )
        for label, initial_host, target_host, supplied_san, expected in cases:
            with self.subTest(identity=label), tempfile.TemporaryDirectory(dir=self.temp.name) as directory:
                self.install = Path(directory)
                self.assertEqual(self.prepare(initial_host).returncode, 0)
                cert = self.install / "docker/nginx/ssl/fullchain.pem"
                key = self.install / "docker/nginx/ssl/privkey.pem"
                if supplied_san:
                    # The target CN deliberately cannot rescue an incorrect SAN.
                    result = run("openssl", "req", "-x509", "-nodes", "-newkey", "rsa:2048", "-days", "1",
                                 "-subj", "/CN=" + target_host, "-addext", "subjectAltName=" + supplied_san,
                                 "-keyout", str(key), "-out", str(cert))
                    self.assertEqual(result.returncode, 0, "Cannot provision operator test pair")
                files = (cert, key, self.install / ".env")
                before = [path.read_bytes() for path in files]
                result = self.prepare(target_host)
                self.assertEqual(result.returncode, expected, "Installer certificate identity decision is wrong")
                self.assertTrue(all(path.read_bytes() == original for path, original in zip(files, before)),
                                "Certificate, key or .env changed during identity validation")

    def test_nginx_fails_without_valid_tls_or_with_injected_config(self):
        for broken in ("missing", "invalid", "mismatch", "host", "port"):
            with self.subTest(broken=broken):
                self.assertEqual(self.prepare().returncode, 0)
                cert = self.install / "docker/nginx/ssl/fullchain.pem"
                original = cert.read_bytes()
                if broken == "missing":
                    cert.unlink()
                elif broken == "invalid":
                    cert.write_text("invalid\n")
                elif broken == "mismatch":
                    other = self.install / "other.pem"
                    result = run("openssl", "req", "-x509", "-nodes", "-newkey", "rsa:2048", "-days", "1",
                                 "-subj", "/CN=other", "-keyout", str(self.install / "other.key"), "-out", str(other))
                    self.assertEqual(result.returncode, 0)
                    cert.write_bytes(other.read_bytes())
                    self.assertNotEqual(self.prepare().returncode, 0)
                service = self.compose()
                name = "pwa163-" + uuid.uuid4().hex[:12]
                self.containers.append(name)
                args = ["docker", "run", "--rm", "--name", name, "--network", self.network, "--memory", "128m",
                        "--add-host", "backend:127.0.0.1", "--add-host", "frontend:127.0.0.1",
                        "-e", "PIPING_PUBLIC_HOST=" + ("x;return 200" if broken == "host" else "localhost"),
                        "-e", "PIPING_HTTPS_PORT=" + ("443;return 200" if broken == "port" else "443")]
                for volume in service["volumes"]:
                    args += ["-v", volume["source"] + ":" + volume["target"] + (":ro" if volume.get("read_only") else "")]
                result = run(*args, service["image"])
                self.assertEqual(result.returncode, 1, "Broken edge must exit, not serve HTTP-only")
                cert.write_bytes(original)


if __name__ == "__main__":
    unittest.main()
