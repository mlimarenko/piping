"""Public artifact and bootstrap behavior with real Git, HTTP and tar archives."""

from __future__ import annotations

import copy
import hashlib
import io
import os
import shutil
import socket
import ssl
import subprocess
import tarfile
import tempfile
import threading
import unittest
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from unittest.mock import patch

from scripts import public_release as public

ROOT = Path(__file__).resolve().parents[2]


class HTTPFixture(public.Network):
    """Transport-only URL remapping; production URL construction stays intact."""

    def __init__(self, tls_context=None):
        self.responses = {}
        self.requests = []
        self.posts = []
        self.post_responses = {}
        self.headers = []
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                url = urllib.parse.unquote(self.path[1:])
                if not url.startswith("https://"):
                    url = "https://" + self.headers["Host"] + self.path
                fixture.requests.append(url)
                fixture.headers.append((url, dict(self.headers)))
                body = fixture.responses.get(url)
                if body is None:
                    self.send_error(404)
                    return
                if callable(body):
                    body = body(self.headers)
                if isinstance(body, int):
                    self.send_error(body)
                    return
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_POST(self):
                url = urllib.parse.unquote(self.path[1:])
                body = self.rfile.read(int(self.headers["Content-Length"]))
                fixture.posts.append((url, body))
                fixture.headers.append((url, dict(self.headers)))
                result = fixture.post_responses.get(url, b"{}")
                if isinstance(result, int):
                    self.send_error(result)
                    return
                if callable(result):
                    result = result(body)
                self.send_response(201)
                self.end_headers()
                self.wfile.write(result)

            def log_message(self, *args):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        if tls_context is not None:
            self.server.socket = tls_context.wrap_socket(self.server.socket, server_side=True)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def request(self, url, *, limit=1024 * 1024, headers=None, data=None, method=None):
        local = f"http://127.0.0.1:{self.server.server_port}/" + urllib.parse.quote(url, safe="")
        request = urllib.request.Request(local, headers=headers or {}, data=data, method=method)
        with urllib.request.urlopen(request, timeout=5) as response:
            body = response.read(limit + 1)
            public.require(len(body) <= limit, "network response exceeds limit")
            return body


class PublicArtifactTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="piping-public-test-")
        self.addCleanup(self.temp.cleanup)
        self.enterContext(patch.dict(os.environ, HOME=self.temp.name))
        self.root = Path(self.temp.name) / "source"
        self.root.mkdir()
        self.source = {"schema": "piping.public-source/v1", "version": "1.2.3",
                       "commit": "a" * 40, "pipeline_id": "123"}
        # Exercise the native installer list rather than copying its file model.
        for name in public.payload_paths(ROOT):
            if name == "env/images.env":
                continue
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, target)
            if name == "install.sh":
                target.write_text(target.read_text().replace("__PIPING_PUBLIC_VERSION__", "1.2.3"))
            # Git records regular-file/executable modes, not the workstation's
            # group-write umask. Model a committed checkout's canonical modes.
            target.chmod(0o755 if target.stat().st_mode & 0o111 else 0o644)
        (self.root / "env").mkdir(exist_ok=True)
        (self.root / "env/public-source.json").write_bytes(public.canonical(self.source))
        (self.root / "env/public-source.json").chmod(0o644)
        for args in [("init", "--quiet"), ("add", "."),
                     ("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                      "commit", "--quiet", "-m", "Public snapshot"), ("tag", "1.2.3")]:
            subprocess.run(["git", "-C", str(self.root), *args], check=True, capture_output=True, timeout=20)
        self.commit = public.run(["git", "rev-parse", "HEAD"], cwd=self.root).decode().strip()
        self.images = {role: {"repository": repo, "digest": "sha256:" + str(i) * 64}
                       for i, (role, repo) in enumerate(public.IMAGES.items(), 1)}
        self.http = HTTPFixture()
        self.addCleanup(self.http.close)
        self.api = f"https://api.github.com/repos/{public.REPOSITORY}/"
        self.download = f"https://github.com/{public.REPOSITORY}/releases/download/1.2.3/"
        self.release = {"id": 42, "tag_name": "1.2.3", "draft": False, "prerelease": False,
                        "published_at": "2026-10-20T00:00:00Z", "assets": []}
        self.respond(self.api + "commits/1.2.3", {"sha": self.commit})
        self.respond(self.api + "releases/tags/1.2.3", self.release)
        self.respond(f"https://raw.githubusercontent.com/{public.REPOSITORY}/1.2.3/env/public-source.json", self.source)

    def respond(self, url, value):
        self.http.responses[url] = public.canonical(value)

    def archive(self):
        return public.build_archive(self.root, self.source, self.images)

    def manifest(self, raw=None):
        raw = self.archive() if raw is None else raw
        return {"schema": "piping.public-release/v1", "version": "1.2.3", "public_commit": self.commit,
                "source_commit": self.source["commit"], "pipeline_id": "123", "platform": "linux/amd64",
                "images": self.images, "installation": {"name": public.archive_name("1.2.3"),
                "size_bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}}

    def ready(self, raw=None):
        raw = self.archive() if raw is None else raw
        manifest = self.manifest(raw)
        self.respond(self.download + public.MANIFEST, manifest)
        self.http.responses[self.download + public.archive_name("1.2.3")] = raw
        self.release["assets"] = [{"name": public.MANIFEST}, {"name": public.archive_name("1.2.3")}]
        self.respond(self.api + "releases/tags/1.2.3", self.release)
        return manifest

    def extract(self, raw, manifest=None):
        destination = Path(self.temp.name) / "extracted"
        destination.mkdir(exist_ok=True)
        public.extract_archive(raw, manifest or self.manifest(raw), destination)
        return destination

    def test_archive_deterministic_complete_modes_and_real_installer_help(self):
        raw = self.archive()
        self.assertEqual(raw, self.archive())
        destination = self.extract(raw)
        self.assertEqual((destination / "env/images.env").read_bytes(), public.images_env(self.images))
        self.assertEqual((destination / "install.sh").stat().st_mode & 0o777,
                         (self.root / "install.sh").stat().st_mode & 0o777)
        result = subprocess.run(["bash", str(destination / "install.sh"), "--help"],
                                check=False, capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Install or update Piping", result.stdout)
        self.assertFalse((destination / ".env").exists())

    def test_manifest_rejects_wrong_version_source_unknown_repository_and_missing_role(self):
        mutations = [("version", "1.2.4"), ("source_commit", "b" * 40), ("public_commit", "b" * 40),
                     ("pipeline_id", "124"), ("platform", "linux/arm64"), ("extra", True)]
        for key, value in mutations:
            with self.subTest(key=key), self.assertRaises(ValueError):
                manifest = copy.deepcopy(self.manifest())
                manifest[key] = value
                public.validate_manifest(manifest, "1.2.3", self.source, self.commit)
        for mutate in [lambda x: x["images"].pop("frontend"),
                       lambda x: x["images"]["backend"].update(repository="docker.io/foreign/backend")]:
            manifest = copy.deepcopy(self.manifest())
            mutate(manifest)
            with self.assertRaises(ValueError):
                public.validate_manifest(manifest, "1.2.3", self.source, self.commit)

    def test_duplicate_json_and_unrendered_bootstrap_refused(self):
        with self.assertRaises(ValueError):
            public.decode(b'{"version":"1.2.3","version":"1.2.4"}')
        # Anonymous stdin must not discover a source checkout in its caller's
        # directory. Keep Docker off PATH so this regression cannot use a daemon.
        tools = Path(self.temp.name) / "stdin-tools"
        tools.mkdir()
        (tools / "dirname").symlink_to(shutil.which("dirname"))
        result = subprocess.run([shutil.which("bash")], input=(ROOT / "install.sh").read_bytes(),
                                env={**os.environ, "PATH": str(tools)}, cwd=ROOT,
                                check=False, capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"published version tag", result.stderr)

    def test_archive_checksum_missing_symlink_and_unsafe_members(self):
        with self.assertRaises(ValueError):
            self.extract(self.archive() + b"wrong", self.manifest())
        (self.root / "docker-compose.yml").unlink()
        with self.assertRaises(ValueError):
            self.archive()
        (self.root / "docker-compose.yml").symlink_to(ROOT / "docker-compose.yml")
        with self.assertRaises(ValueError):
            self.archive()
        for name, kind in [("../escape", tarfile.REGTYPE), ("/escape", tarfile.REGTYPE),
                           (".env", tarfile.REGTYPE), ("linked", tarfile.SYMTYPE),
                           ("linked", tarfile.LNKTYPE), ("fifo", tarfile.FIFOTYPE)]:
            with self.subTest(name=name, kind=kind):
                output = io.BytesIO()
                with tarfile.open(fileobj=output, mode="w:gz") as archive:
                    member = tarfile.TarInfo(name)
                    member.type = kind
                    member.linkname = "../escape"
                    archive.addfile(member)
                raw = output.getvalue()
                manifest = {"installation": {"size_bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}}
                with self.assertRaises(ValueError):
                    self.extract(raw, manifest)
        self.assertFalse((Path(self.temp.name) / "escape").exists())

    def test_archive_cannot_substitute_cohort_snapshot_or_omit_payload(self):
        original = self.archive()
        for changed in ["env/images.env", "env/public-source.json", "docker-compose.yml", "duplicate"]:
            with self.subTest(changed=changed):
                output = io.BytesIO()
                with (tarfile.open(fileobj=io.BytesIO(original), mode="r:gz") as source,
                      tarfile.open(fileobj=output, mode="w:gz") as target):
                    for item in source:
                        if item.name == changed == "docker-compose.yml":
                            continue
                        data = source.extractfile(item).read()
                        if item.name == changed:
                            data = (b"PIPING_BACKEND_IMAGE=docker.io/foreign/backend:latest\n"
                                    if changed == "env/images.env" else
                                    public.canonical(dict(self.source, commit="b" * 40)))
                        item.size = len(data)
                        target.addfile(item, io.BytesIO(data))
                        if changed == "duplicate" and item.name == "install.sh":
                            target.addfile(item, io.BytesIO(data))
                raw = output.getvalue()
                destination = Path(self.temp.name) / changed.replace("/", "-")
                destination.mkdir()
                with self.assertRaises(ValueError):
                    public.extract_archive(raw, self.manifest(raw), destination)

    def test_decompression_is_bounded_before_tar_header_parsing(self):
        import gzip

        raw = gzip.compress(b"x" * (1024 * 1024 + 2048))
        manifest = {"installation": {"size_bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}}
        with patch.object(public, "MAX_EXPANDED", 1024), self.assertRaises(ValueError):
            self.extract(raw, manifest)

    def test_http_install_uses_verified_archive_and_existing_installer(self):
        self.ready()
        # This executes the actual installer, whose --help exits before Docker
        # or installation mutation; HTTP, checksum and extraction remain real.
        public.install("1.2.3", ["--help"], self.http)
        self.assertIn(self.download + public.MANIFEST, self.http.requests)
        self.assertIn(self.download + public.archive_name("1.2.3"), self.http.requests)

    def test_public_destination_and_argument_refusals_precede_downloads(self):
        target = Path(self.temp.name) / "destination"
        target.mkdir()
        (target / "docker-compose.yml").write_text("partial installation\n")
        cases = [["--install-dir", str(target)], ["--install-dir", "/"],
                 ["--install-dir", self.temp.name], ["--install-dir"],
                 ["--source-base-url", "https://unapproved.example"],
                 ["--frontend-image", "unapproved:latest"]]
        linked = Path(self.temp.name) / "linked"
        linked.symlink_to(target, target_is_directory=True)
        cases.append(["--install-dir", str(linked / "child")])
        for arguments in cases:
            with self.subTest(arguments=arguments), self.assertRaises(ValueError):
                public.install("1.2.3", arguments, self.http)
        (target / ".env").symlink_to(target / "docker-compose.yml")
        with self.assertRaises(ValueError):
            public.install("1.2.3", ["--install-dir", str(target)], self.http)
        self.assertEqual(self.http.requests, [])
        self.assertEqual((target / "docker-compose.yml").read_text(), "partial installation\n")

    def test_standalone_partial_layout_and_argument_errors_do_not_bootstrap(self):
        partial = Path(self.temp.name) / "partial"
        partial.mkdir()
        script = partial / "install.sh"
        script.write_text((ROOT / "install.sh").read_text().replace("__PIPING_PUBLIC_VERSION__", "1.2.3"))
        (partial / "docker-compose.yml").write_text("retained\n")
        for arguments, diagnostic in [([], b"Incomplete installation/source layout"),
                                      (["--install-dir"], b"Missing option value"),
                                      (["--unknown", "value"], b"Unknown option")]:
            result = subprocess.run(["bash", str(script), *arguments], capture_output=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(diagnostic, result.stderr)
        linked = partial / "linked.sh"
        linked.symlink_to(script)
        result = subprocess.run(["bash", str(linked)], capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"must not be a symlink", result.stderr)
        self.assertEqual((partial / "docker-compose.yml").read_text(), "retained\n")

    def test_native_layout_fresh_retry_retains_settings_tls_and_payload_fingerprint(self):
        source = self.extract(self.archive())
        destination = Path(self.temp.name) / "layout"
        command = ['bash', '-c', 'source "$1/install.sh"; INSTALL_DIR="$2"; '
                   'PROJECT_NAME=pip144-fixture; PUBLIC_HOST=localhost; '
                   'prepare_layout; (cd "$INSTALL_DIR" && sha256sum --check --status .fresh-install-in-progress)',
                   'layout', str(source), str(destination)]
        result = subprocess.run(command, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 0, "Native layout preparation failed")
        protected = [".env", "docker/nginx/ssl/fullchain.pem", "docker/nginx/ssl/privkey.pem",
                     ".fresh-install-in-progress", "env/public-source.json"]
        fingerprints = {name: hashlib.sha256((destination / name).read_bytes()).hexdigest()
                        for name in protected}
        result = subprocess.run(command, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 0, "Native interrupted-fresh retry failed")
        self.assertEqual(fingerprints, {name: hashlib.sha256((destination / name).read_bytes()).hexdigest()
                                       for name in protected})
        self.assertEqual((destination / ".env").stat().st_mode & 0o777, 0o600)
        self.assertEqual((destination / "docker/nginx/ssl/privkey.pem").stat().st_mode & 0o777, 0o600)
        self.assertIn(b"COMPOSE_PROJECT_NAME=pip144-fixture\n", (destination / ".env").read_bytes())
        self.assertIn(b"PIPING_PUBLIC_HOST=localhost\n", (destination / ".env").read_bytes())

    def test_root_public_descriptor_binding_refuses_wrong_rendered_version(self):
        source = self.extract(self.archive())
        script = source / "install.sh"
        script.write_text(script.read_text().replace("PUBLIC_VERSION='1.2.3'", "PUBLIC_VERSION='1.2.4'"))
        result = subprocess.run(["bash", str(script)], capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"Public installation version differs", result.stderr)
        self.assertFalse((source / ".env").exists())

    def test_complete_source_rejects_a_symlinked_cohort_directory(self):
        source = self.extract(self.archive())
        (source / "env").rename(source / "cohort")
        (source / "env").symlink_to("cohort", target_is_directory=True)
        result = subprocess.run(["bash", str(source / "install.sh")], capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"must not contain symlinks", result.stderr)
        self.assertFalse((source / ".env").exists())

    def test_rendered_root_stdin_verifies_real_tls_package_without_using_caller_source(self):
        # Only transport endpoints are remapped. Both curl and the downloaded,
        # unmodified Python verifier retain their real HTTPS validation paths.
        cert = Path(self.temp.name) / "cert.pem"
        key = Path(self.temp.name) / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                        "-days", "1", "-keyout", str(key), "-out", str(cert), "-subj", "/CN=github.com",
                        "-addext", "subjectAltName=DNS:github.com,DNS:api.github.com,DNS:raw.githubusercontent.com"],
                       check=True, capture_output=True, timeout=20)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        tls = HTTPFixture(context)
        self.addCleanup(tls.close)
        self.ready()
        tls.responses.update(self.http.responses)
        verifier_url = "https://raw.githubusercontent.com/mlimarenko/piping/1.2.3/scripts/public_release.py"
        tls.responses[verifier_url] = (ROOT / "scripts/public_release.py").read_bytes()
        tools = Path(self.temp.name) / "tools"
        tools.mkdir()
        for name in ("bash", "python3", "dirname", "mktemp", "rm", "rmdir", "openssl", "realpath"):
            (tools / name).symlink_to(shutil.which(name))
        curl = tools / "curl"
        curl.write_text(f'#!/bin/sh\nexec {shutil.which("curl")} --noproxy "*" --cacert "{cert}" '
                        f'--connect-to raw.githubusercontent.com:443:127.0.0.1:{tls.server.server_port} "$@"\n')
        curl.chmod(0o755)
        transport = Path(self.temp.name) / "transport"
        transport.mkdir()
        (transport / "sitecustomize.py").write_text(
            "import socket\noriginal = socket.create_connection\n"
            "def connect(address, *args, **kwargs):\n"
            "    assert address[0] in {'github.com', 'api.github.com', 'raw.githubusercontent.com'}\n"
            f"    return original(('127.0.0.1', {tls.server.server_port}), *args, **kwargs)\n"
            "socket.create_connection = connect\n")
        environment = {k: v for k, v in os.environ.items()
                       if "proxy" not in k.lower() and k not in {"GH_TOKEN", "GITHUB_PUBLIC_TOKEN"}}
        environment.update(PATH=str(tools), PYTHONPATH=str(transport), SSL_CERT_FILE=str(cert))
        destination = Path(self.temp.name) / "fresh"
        result = subprocess.run([shutil.which("bash"), "-s", "--", "--install-dir", str(destination)],
                                input=(self.root / "install.sh").read_bytes(), cwd=ROOT,
                                env=environment, capture_output=True, timeout=30)
        # Docker is deliberately absent: prove acquisition and actual extracted
        # engine selection, not a successful container/database installation.
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"Required command is missing: docker", result.stderr)
        self.assertNotIn(b"unbound variable", result.stderr)
        self.assertIn(verifier_url, tls.requests)
        self.assertIn(self.download + public.MANIFEST, tls.requests)
        self.assertIn(self.download + public.archive_name("1.2.3"), tls.requests)
        self.assertFalse(destination.exists())
        # The same canonical root from a complete source payload never acquires
        # a public package, even though its public version is rendered.
        tls.requests.clear()
        (self.root / "env/images.env").write_bytes(public.images_env(self.images))
        result = subprocess.run([shutil.which("bash"), str(self.root / "install.sh")],
                                env=environment, capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"Required command is missing: docker", result.stderr)
        self.assertEqual(tls.requests, [])

    def test_incomplete_and_wrong_snapshot_never_invoke_installer(self):
        with patch.object(public.subprocess, "run") as engine:
            with self.assertRaises(urllib.error.HTTPError) as caught:
                public.install("1.2.3", [], self.http)
            caught.exception.close()
            engine.assert_not_called()
        self.ready()
        wrong = dict(self.source, commit="b" * 40)
        self.respond(f"https://raw.githubusercontent.com/{public.REPOSITORY}/1.2.3/env/public-source.json", wrong)
        with patch.object(public.subprocess, "run") as engine:
            with self.assertRaises(ValueError):
                public.install("1.2.3", [], self.http)
            engine.assert_not_called()

    def registry(self, role, *, changes=None, absent=False):
        repository = public.IMAGES[role].removeprefix("docker.io/")
        self.respond("https://auth.docker.io/token?service=registry.docker.io&scope=repository:" + repository + ":pull",
                     {"token": "fixture-anonymous-pull-token"})
        base = "https://registry-1.docker.io/v2/" + repository
        if absent:
            return
        config = {"architecture": "amd64", "os": "linux",
                  "config": {"Labels": public.labels(self.source, self.commit)}}
        if changes:
            changes(config)
        raw = public.canonical(config)
        config_digest = "sha256:" + hashlib.sha256(raw).hexdigest()
        self.http.responses[base + "/blobs/" + config_digest] = raw
        manifest = {"schemaVersion": 2, "config": {"digest": config_digest}, "layers": []}
        raw = public.canonical(manifest)
        digest = "sha256:" + hashlib.sha256(raw).hexdigest()
        self.http.responses[base + "/manifests/1.2.3"] = raw
        self.images[role]["digest"] = digest

    def test_registry_provenance_and_absent_tag_boundary(self):
        self.registry("backend", absent=True)
        self.assertIsNone(public.registry_image(self.http, "backend", self.source, self.commit))
        self.registry("backend")
        self.assertEqual(public.registry_image(self.http, "backend", self.source, self.commit), self.images["backend"])
        for change in [lambda x: x["config"]["Labels"].update({"org.opencontainers.image.revision": "b" * 40}),
                       lambda x: x["config"]["Labels"].update({"space.piping.source.commit": "b" * 40}),
                       lambda x: x.update(architecture="arm64")]:
            self.registry("backend", changes=change)
            with self.assertRaises(ValueError):
                public.registry_image(self.http, "backend", self.source, self.commit)

    def test_complete_publication_noop_and_conflicting_asset_refused(self):
        for role in public.IMAGES:
            self.registry(role)
        self.ready()
        with patch.dict(os.environ, {"GITHUB_OUTPUT": str(Path(self.temp.name) / "outputs")}):
            public.prepare(self.root, "1.2.3", self.http)
        self.assertIn("complete=true", (Path(self.temp.name) / "outputs").read_text())
        public.finalize(self.root, "1.2.3", self.http, {})
        self.assertEqual(self.http.posts, [])
        self.http.responses[self.download + public.archive_name("1.2.3")] = b"conflicting archive"
        with self.assertRaises(ValueError):
            public.finalize(self.root, "1.2.3", self.http, {})
        self.assertEqual(self.http.posts, [])

    def test_publication_archive_first_manifest_last_and_partial_retry(self):
        for role in public.IMAGES:
            self.registry(role)
        with patch.dict(os.environ, {"GH_TOKEN": "fixture-only"}):
            public.finalize(self.root, "1.2.3", self.http, {})
        self.assertEqual(len(self.http.posts), 2)
        self.assertTrue(self.http.posts[0][0].endswith("name=" + public.archive_name("1.2.3")))
        self.assertTrue(self.http.posts[1][0].endswith("name=" + public.MANIFEST))
        # Resume a release with an already uploaded archive; never overwrite it.
        self.release["assets"] = [{"name": public.archive_name("1.2.3")}]
        self.respond(self.api + "releases/tags/1.2.3", self.release)
        self.http.responses[self.download + public.archive_name("1.2.3")] = self.http.posts[0][1]
        self.http.posts.clear()
        with patch.dict(os.environ, {"GH_TOKEN": "fixture-only"}):
            public.finalize(self.root, "1.2.3", self.http, {})
        self.assertEqual(len(self.http.posts), 1)
        self.assertTrue(self.http.posts[0][0].endswith("name=" + public.MANIFEST))

    def test_partial_prepare_reuses_only_matching_tags(self):
        self.registry("backend")
        self.registry("frontend", absent=True)
        self.registry("ansible-runner", absent=True)
        output = Path(self.temp.name) / "outputs"
        with patch.dict(os.environ, {"GITHUB_OUTPUT": str(output)}):
            public.prepare(self.root, "1.2.3", self.http)
        self.assertIn("build_backend=false", output.read_text())
        self.assertIn("build_frontend=true", output.read_text())
        self.assertIn("complete=false", output.read_text())

    def test_bare_rerun_keeps_installed_engine_and_saved_files(self):
        self.ready()
        installed = Path(self.temp.name) / "installed"
        installed.mkdir()
        public.extract_archive(self.archive(), self.manifest(), installed)
        # A saved cohort intentionally differs from the downloaded release.
        (installed / ".env").write_text("SAVED_SETTINGS=retained\n")
        (installed / ".images.env").write_text("SAVED_COHORT=retained\n")
        before = {name: (installed / name).read_bytes() for name in [".env", ".images.env", "install.sh"]}
        native_run = subprocess.run
        calls = []

        def observe(arguments, **kwargs):
            if len(arguments) >= 2 and arguments[0] == "bash" and str(arguments[1]).endswith("install.sh"):
                calls.append(arguments)
            return native_run(arguments, **kwargs)

        with patch.object(public.subprocess, "run", side_effect=observe):
            requests_before = list(self.http.requests)
            public.install("1.2.3", ["--install-dir", str(installed), "--help"], self.http)
        self.assertEqual(calls[-1][1], str(installed / "install.sh"))
        self.assertEqual(before, {name: (installed / name).read_bytes() for name in before})
        self.assertEqual(self.http.requests, requests_before)

    def test_incomplete_image_cohort_and_foreign_build_output_never_upload(self):
        self.registry("backend")
        self.registry("frontend", absent=True)
        self.registry("ansible-runner", absent=True)
        with self.assertRaises(ValueError):
            public.finalize(self.root, "1.2.3", self.http, {})
        self.assertEqual(self.http.posts, [])
        for role in ["frontend", "ansible-runner"]:
            self.registry(role)
        with self.assertRaises(ValueError):
            public.finalize(self.root, "1.2.3", self.http, {"backend": "sha256:" + "f" * 64})
        self.assertEqual(self.http.posts, [])

    def test_registry_index_binds_single_runnable_platform_and_config_digest(self):
        self.registry("backend")
        base = "https://registry-1.docker.io/v2/pipingspace/piping-backend"
        raw = self.http.responses[base + "/manifests/1.2.3"]
        child = "sha256:" + hashlib.sha256(raw).hexdigest()
        self.http.responses[base + "/manifests/" + child] = raw
        index = {"schemaVersion": 2, "manifests": [
            {"digest": child, "platform": {"os": "linux", "architecture": "amd64"}},
            {"digest": "sha256:" + "e" * 64, "platform": {"os": "unknown", "architecture": "unknown"}}]}
        self.respond(base + "/manifests/1.2.3", index)
        expected = "sha256:" + hashlib.sha256(public.canonical(index)).hexdigest()
        self.assertEqual(public.registry_image(self.http, "backend", self.source, self.commit)["digest"], expected)
        index["manifests"].append({"digest": child, "platform": {"os": "linux", "architecture": "arm64"}})
        self.respond(base + "/manifests/1.2.3", index)
        with self.assertRaises(ValueError):
            public.registry_image(self.http, "backend", self.source, self.commit)
        self.http.responses[base + "/manifests/1.2.3"] = raw
        config_digest = public.decode(raw)["config"]["digest"]
        self.http.responses[base + "/blobs/" + config_digest] = b"wrong config bytes"
        with self.assertRaises(ValueError):
            public.registry_image(self.http, "backend", self.source, self.commit)

    def test_published_stable_release_and_selected_git_tag_required(self):
        for key in ["draft", "prerelease"]:
            self.respond(self.api + "releases/tags/1.2.3", dict(self.release, **{key: True}))
            with self.assertRaises(ValueError):
                public.context(self.root, "1.2.3", self.http)
        self.respond(self.api + "releases/tags/1.2.3", self.release)
        self.respond(self.api + "commits/1.2.3", {"sha": "b" * 40})
        with self.assertRaises(ValueError):
            public.context(self.root, "1.2.3", self.http)

    def channel_fixture(self):
        base = "https://hub.docker.com/v2/"
        self.http.post_responses[base + "auth/token"] = public.canonical({"access_token": "fixture.access.token"})
        for repository in public.IMAGES.values():
            name = repository.rsplit("/", 1)[1]
            self.respond(base + "namespaces/pipingspace/repositories/" + name, self.channel_info(name))
        return base

    def channel_info(self, name, *, initialized=False):
        return {"name": name, "namespace": "pipingspace", "is_private": False, "repository_type": "image",
                "status": 0 if initialized else 1, "status_description": "initialized" if initialized else "Active",
                "is_automated": False, "permissions": {"read": True, "write": True, "admin": False},
                "media_types": [], "content_types": [], "storage_size": None}

    def respond_initialized_channel(self, base, name):
        def metadata(headers):
            value = self.channel_info(name, initialized=True)
            # Real anonymous empty-channel metadata grants read, not write.
            value["permissions"]["write"] = headers.get("Authorization") == "Bearer fixture.access.token"
            return public.canonical(value)

        self.http.responses[base + "namespaces/pipingspace/repositories/" + name] = metadata

    def initialize(self):
        with patch.dict(os.environ, {"DOCKERHUB_USERNAME": "fixture-user", "DOCKERHUB_TOKEN": "fixture-pat"}):
            public.initialize_channels(self.root, "1.2.3", self.http)

    def test_channel_initialization_existing_public_repositories_does_not_mutate(self):
        base = self.channel_fixture()
        self.initialize()
        self.assertEqual([url for url, _ in self.http.posts], [base + "auth/token"])
        lookups = [(url, headers) for url, headers in self.http.headers if "/namespaces/" in url]
        self.assertEqual(sum("Authorization" not in headers for _, headers in lookups), 3)
        self.assertTrue(all(headers.get("Authorization") == "Bearer fixture.access.token"
                            for _, headers in lookups if "Authorization" in headers))

    def test_channel_initialization_creates_only_exact_missing_public_channels(self):
        base = self.channel_fixture()
        for image in public.IMAGES.values():
            self.http.responses.pop(base + "namespaces/pipingspace/repositories/" + image.rsplit("/", 1)[1])

        def create(raw):
            value = public.decode(raw)
            self.assertEqual(value["namespace"], "pipingspace")
            self.assertEqual(value["registry"], "docker.io")
            self.assertIs(value["is_private"], False)
            metadata = self.channel_info(value["name"], initialized=True)
            self.respond_initialized_channel(base, value["name"])
            return public.canonical(metadata)

        self.http.post_responses[base + "namespaces/pipingspace/repositories"] = create
        self.initialize()
        creates = [public.decode(body)["name"] for url, body in self.http.posts if "/namespaces/" in url]
        self.assertEqual(set(creates), {image.rsplit("/", 1)[1] for image in public.IMAGES.values()})
        self.assertEqual(len(creates), 3)

    def test_channel_initialization_refuses_private_foreign_and_unwritable_without_create(self):
        base = self.channel_fixture()
        first = next(iter(public.IMAGES.values())).rsplit("/", 1)[1]
        for initialized in [False, True]:
            for changes in [{"is_private": True}, {"namespace": "foreign"}, {"repository_type": "model"},
                            {"permissions": {"read": True, "write": False}}, {"is_automated": True}]:
                with self.subTest(initialized=initialized, changes=changes):
                    self.respond(base + "namespaces/pipingspace/repositories/" + first,
                                 dict(self.channel_info(first, initialized=initialized), **changes))
                    with self.assertRaises(public.ChannelError):
                        self.initialize()
                    self.assertFalse(any("/namespaces/" in url for url, _ in self.http.posts))

    def test_channel_initialization_resumes_existing_empty_backend_and_creates_remaining(self):
        base = self.channel_fixture()
        backend = "piping-backend"
        remaining = {"piping-frontend", "piping-ansible-runner"}
        self.respond_initialized_channel(base, backend)
        for name in remaining:
            self.http.responses.pop(base + "namespaces/pipingspace/repositories/" + name)

        def create(raw):
            value = public.decode(raw)
            self.assertIn(value["name"], remaining)
            self.assertIs(value["is_private"], False)
            self.respond_initialized_channel(base, value["name"])
            return public.canonical(self.channel_info(value["name"], initialized=True))

        self.http.post_responses[base + "namespaces/pipingspace/repositories"] = create
        self.initialize()
        creates = [public.decode(body)["name"] for url, body in self.http.posts if "/namespaces/" in url]
        self.assertEqual(set(creates), remaining)
        self.assertEqual(len(creates), 2)
        # A second pass sees all three initialized channels and creates none.
        self.http.posts.clear()
        self.initialize()
        self.assertFalse(any("/namespaces/" in url for url, _ in self.http.posts))

    def test_channel_initialization_refuses_disabled_unknown_and_inconsistent_status(self):
        base = self.channel_fixture()
        first = "piping-backend"
        for status, description in [(2, "disabled"), (-1, "disabled"), (3, "unknown"),
                                    (False, "initialized"), (True, "Active"), ("0", "initialized"),
                                    (0, "disabled"), (0, "Active"), (1, "initialized"), (1, None)]:
            with self.subTest(status=status, description=description):
                self.respond(base + "namespaces/pipingspace/repositories/" + first,
                             dict(self.channel_info(first), status=status, status_description=description))
                with self.assertRaises(public.ChannelError):
                    self.initialize()
                self.assertFalse(any("/namespaces/" in url for url, _ in self.http.posts))

    def test_channel_initialization_does_not_interpret_401_or_403_as_missing(self):
        base = self.channel_fixture()
        first = next(iter(public.IMAGES.values())).rsplit("/", 1)[1]
        for code in [401, 403]:
            self.http.responses[base + "namespaces/pipingspace/repositories/" + first] = code
            with self.assertRaises(public.ChannelError) as caught:
                self.initialize()
            self.assertIn(str(code), str(caught.exception))
            self.assertIn("creation/write rights", str(caught.exception))
            self.assertNotIn("fixture-pat", str(caught.exception))
            self.assertFalse(any("/namespaces/" in url for url, _ in self.http.posts))

    def test_channel_create_permission_denial_or_existing_hidden_name_is_actionable(self):
        base = self.channel_fixture()
        first = next(iter(public.IMAGES.values())).rsplit("/", 1)[1]
        self.http.responses.pop(base + "namespaces/pipingspace/repositories/" + first)
        for code in [403, 409]:
            self.http.post_responses[base + "namespaces/pipingspace/repositories"] = code
            with self.assertRaises(public.ChannelError) as caught:
                self.initialize()
            self.assertIn("visibility is never changed", str(caught.exception))
            self.assertIn(str(code), str(caught.exception))


class NetworkBoundaryTests(unittest.TestCase):
    """Native urllib redirect behavior over local TLS, without external traffic."""

    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="piping-network-test-")
        root = Path(cls.temp.name)
        cls.addClassCleanup(cls.temp.cleanup)
        cert, key = root / "certificate.pem", root / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                        "-keyout", str(key), "-out", str(cert), "-subj", "/CN=github.com", "-addext",
                        "subjectAltName=DNS:github.com,DNS:release-assets.githubusercontent.com,DNS:hub.docker.com"],
                       check=True, capture_output=True, timeout=20)
        server_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        server_context.load_cert_chain(cert, key)
        cls.client_context = ssl.create_default_context(cafile=str(cert))
        cls.observed = []

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                self.respond()

            def do_POST(self):
                self.rfile.read(int(self.headers.get("Content-Length", "0")))
                self.respond()

            def respond(self):
                cls.observed.append((self.path, dict(self.headers)))
                redirects = {"/same": "https://github.com/done",
                             "/cdn": "https://release-assets.githubusercontent.com/done",
                             "/downgrade": "http://github.com/done",
                             "/foreign": "https://unrelated.example/done",
                             "/post": "https://hub.docker.com/done"}
                if self.path in redirects:
                    self.send_response(302)
                    self.send_header("Location", redirects[self.path])
                    self.end_headers()
                else:
                    self.send_response(200)
                    self.end_headers()
                    self.wfile.write(b"verified-local-tls")

            def log_message(self, *args):
                pass

        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.server.socket = server_context.wrap_socket(cls.server.socket, server_side=True)
        cls.thread = threading.Thread(target=cls.server.serve_forever)
        cls.thread.start()
        cls.addClassCleanup(cls.close_server)

    @classmethod
    def close_server(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join(timeout=5)

    def setUp(self):
        self.observed.clear()
        self.network = public.Network(tls_context=self.client_context)
        native_connect = socket.create_connection

        def local_connection(address, *args, **kwargs):
            self.assertIn(address[0], {"github.com", "release-assets.githubusercontent.com", "hub.docker.com"})
            return native_connect(("127.0.0.1", self.server.server_port), *args, **kwargs)

        patcher = patch.object(socket, "create_connection", side_effect=local_connection)
        patcher.start()
        self.addCleanup(patcher.stop)
        proxy = patch.dict(os.environ, {name: "" for name in
                                       ["http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "all_proxy"]})
        proxy.start()
        self.addCleanup(proxy.stop)

    def test_same_origin_keeps_auth_but_approved_release_cdn_gets_no_credentials(self):
        headers = {"Authorization": "Bearer fixture-only", "Cookie": "fixture-only", "X-Api-Key": "fixture-only"}
        self.assertEqual(self.network.request("https://github.com/same", headers=headers), b"verified-local-tls")
        self.assertEqual(self.observed[-1][1].get("Authorization"), "Bearer fixture-only")
        self.assertEqual(self.network.request("https://github.com/cdn", headers=headers), b"verified-local-tls")
        received = {key.lower() for key in self.observed[-1][1]}
        self.assertFalse(received & {"authorization", "cookie", "x-api-key"})
        self.assertEqual(self.observed[-1][1]["Host"], "release-assets.githubusercontent.com")

    def test_downgrade_foreign_origin_and_credential_post_redirect_refused(self):
        for url, options in [("https://github.com/downgrade", {}), ("https://github.com/foreign", {}),
                             ("https://hub.docker.com/post", {"data": b"fixture-credentials", "method": "POST"})]:
            with self.subTest(url=url):
                self.observed.clear()
                with self.assertRaises(ValueError):
                    self.network.request(url, **options)
                self.assertEqual(len(self.observed), 1)
        self.observed.clear()
        with self.assertRaises(ValueError):
            self.network.request("http://github.com/done")
        self.assertEqual(self.observed, [])


if __name__ == "__main__":
    unittest.main()
