"""Real snapshots and local TLS exercise the CI release boundary without GitHub writes."""

from __future__ import annotations

import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import socket
import ssl
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch

from scripts.ci import github_public_release as driver
from scripts.ci import publish_public_release as publisher
from tests.ci import test_public_release as snapshots


class GitHubReleaseTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tls = tempfile.TemporaryDirectory(prefix="pip137-tls-")
        cls.addClassCleanup(cls.tls.cleanup)
        cls.cert, key = Path(cls.tls.name) / "cert.pem", Path(cls.tls.name) / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                        "-days", "1", "-keyout", str(key), "-out", str(cls.cert),
                        "-subj", "/CN=api.github.com", "-addext",
                        "subjectAltName=DNS:api.github.com,DNS:github.com"],
                       check=True, capture_output=True, timeout=20)
        cls.server_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        cls.server_context.load_cert_chain(cls.cert, key)
        cls.client_context = ssl.create_default_context(cafile=str(cls.cert))

    def setUp(self):
        self.fixture = snapshots.PublicReleaseTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.release = None
        self.posts = 0
        self.lost_response = False
        self.post_error = False
        self.actor = {"login": "mlimarenko", "id": 82709956}
        self.repository = {"full_name": driver.REPOSITORY,
                           "html_url": "https://github.com/" + driver.REPOSITORY,
                           "owner": dict(self.actor), "permissions": {"push": True}}
        self.tag_override = None
        self.observed = []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                owner.observed.append((self.path, dict(self.headers)))
                if self.path.startswith("/git/"):
                    if not self.headers.get("Authorization"):
                        self.send_response(401)
                        self.send_header("WWW-Authenticate", 'Basic realm="fixture"')
                        self.send_header("Content-Length", "0")
                        self.end_headers()
                        return
                    self.send_response(200)
                    self.send_header("Content-Type", "text/plain")
                    payload = b"ref: refs/heads/main\n" if self.path.endswith("HEAD") else b""
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                    return
                if self.path == "/user":
                    self.respond(owner.actor)
                elif self.path == "/repos/mlimarenko/piping":
                    self.respond(owner.repository)
                elif self.path == "/repos/mlimarenko/piping/git/ref/tags/1.0.0":
                    commit = owner.fixture.git(owner.fixture.remote, "rev-parse", "1.0.0").strip()
                    self.respond(owner.tag_override or {"ref": "refs/tags/1.0.0",
                                                        "object": {"type": "commit", "sha": commit}})
                elif self.path == "/repos/mlimarenko/piping/releases/tags/1.0.0":
                    self.respond(owner.release, status=404 if owner.release is None else 200)
                else:
                    self.respond({}, status=404)

            def do_POST(self):
                owner.observed.append((self.path, dict(self.headers)))
                owner.assertEqual(self.path, "/repos/mlimarenko/piping/releases")
                owner.assertEqual(self.headers.get("Content-Type"), "application/json")
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                owner.assertFalse(body.pop("generate_release_notes"))
                owner.posts += 1
                body.update(id=41, html_url="https://github.com/mlimarenko/piping/releases/tag/1.0.0")
                body["author"] = dict(owner.actor)
                owner.release = body
                if owner.lost_response:
                    self.close_connection = True
                    return
                if owner.post_error:
                    self.respond({"message": "fixture-only private server body"}, status=500)
                    return
                self.respond(body, status=201)

            def respond(self, body, status=200):
                self.send_response(status)
                self.end_headers()
                self.wfile.write(json.dumps(body).encode())

            def log_message(self, *args):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.socket = self.server_context.wrap_socket(self.server.socket, server_side=True)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.addCleanup(self.close_server)
        native_connect = socket.create_connection

        def local_connection(address, *args, **kwargs):
            self.assertEqual(address, ("api.github.com", 443))
            return native_connect(("127.0.0.1", self.server.server_port), *args, **kwargs)

        self.enterContext(patch.object(socket, "create_connection", side_effect=local_connection))
        self.enterContext(patch.dict(os.environ, {name: "" for name in
                          ("http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "all_proxy")}))
        self.enterContext(patch.dict(os.environ, GITHUB_PUBLIC_TOKEN="fixture-only", GH_TOKEN="wrong-token"))
        native_client = driver.GitHub

        def client(token):
            result = native_client(token)
            result.tls_context = self.client_context
            return result

        self.enterContext(patch.object(driver, "GitHub", side_effect=client))
        self.native_git = publisher.git
        self.remote_calls = []

        def git(directory, *arguments, **kwargs):
            if driver.REMOTE in arguments:
                self.remote_calls.append(arguments)
                self.assertTrue(all("fixture-only" not in argument for argument in arguments))
                auth = kwargs.get("environment")
                self.assertEqual(set(auth), {"GIT_ASKPASS", "GITHUB_PUBLIC_TOKEN"})
                helper = Path(auth["GIT_ASKPASS"])
                self.assertEqual(helper.stat().st_mode & 0o777, 0o700)
                self.assertEqual(helper.parent.stat().st_mode & 0o777, 0o700)
                self.assertNotIn("fixture-only", helper.read_text())
                credentials = self.native_git(directory, "credential", "fill",
                                              data=b"protocol=https\nhost=github.com\n\n",
                                              environment=auth)
                self.assertIn(b"password=fixture-only", credentials)
                arguments = tuple(str(self.fixture.remote) if arg == driver.REMOTE else arg for arg in arguments)
                kwargs.pop("environment")
            else:
                self.assertNotIn("GITHUB_PUBLIC_TOKEN", kwargs.get("environment") or {})
            return self.native_git(directory, *arguments, **kwargs)

        self.enterContext(patch.object(publisher, "git", side_effect=git))
        self.output = self.fixture.root / "publication.json"

    def close_server(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def publish(self, **overrides):
        arguments = dict(source=self.fixture.source, selection=self.fixture.release,
                         commit=self.fixture.sha, pipeline_id="123", output=self.output,
                         scratch_root=self.fixture.scratch)
        arguments.update(overrides)
        return driver.publish(**arguments)

    def test_snapshot_release_and_retry_are_unchanged(self):
        first = self.publish()
        refs = self.fixture.refs()
        second = self.publish()
        self.assertEqual((first["status"], first["release_status"]), ("published", "published"))
        self.assertEqual((second["status"], second["release_status"]), ("unchanged", "unchanged"))
        self.assertEqual(first["public_commit"], second["public_commit"])
        self.assertEqual(self.posts, 1)
        self.assertEqual(self.fixture.refs(), refs)
        self.assertEqual(self.fixture.git(self.fixture.remote, "rev-list", "--count", "main").strip(), "1")
        self.assertEqual(json.loads(self.output.read_text()), second)
        self.assertEqual(list(self.fixture.scratch.iterdir()), [])
        self.assertNotIn("fixture-only", self.output.read_text())
        self.assertNotIn(str(self.fixture.root), self.output.read_text())
        self.assertTrue(all(headers["Authorization"] == "Bearer fixture-only"
                            for _, headers in self.observed))
        identity = self.fixture.git(self.fixture.remote, "show", "-s", "--format=%an <%ae>|%cn <%ce>", "main")
        self.assertEqual(identity.strip(), "|".join(["mlimarenko <82709956+mlimarenko@users.noreply.github.com>"] * 2))

    def test_lost_post_response_reconciles_without_duplicate(self):
        self.lost_response = True
        with self.assertRaises(driver.ReleaseError):
            self.publish()
        refs = self.fixture.refs()
        self.assertEqual(self.posts, 1)
        self.assertFalse(self.output.exists())
        self.lost_response = False
        result = self.publish()
        self.assertEqual(result["release_status"], "unchanged")
        self.assertEqual(self.posts, 1)
        self.assertEqual(self.fixture.refs(), refs)

    def test_post_http_error_is_safe_and_does_not_blind_retry(self):
        self.post_error = True
        with self.assertRaisesRegex(driver.ReleaseError, r"HTTP 500") as error:
            self.publish()
        self.assertNotIn("fixture-only", str(error.exception))
        self.assertEqual(self.posts, 1)
        self.post_error = False
        self.assertEqual(self.publish()["release_status"], "unchanged")
        self.assertEqual(self.posts, 1)

    def test_token_absent_wrong_actor_and_permissions_fail_before_remote_git(self):
        cases = [("", self.actor, True), ("fixture-only", {"login": "other", "id": 82709956}, True),
                 ("fixture-only", {"login": "mlimarenko", "id": 1}, True),
                 ("fixture-only", self.actor, False)]
        for token, actor, push in cases:
            with self.subTest(actor=actor, push=push, absent=not token):
                self.actor = actor
                self.repository["permissions"]["push"] = push
                with patch.dict(os.environ, GITHUB_PUBLIC_TOKEN=token):
                    with self.assertRaises(driver.ReleaseError):
                        self.publish()
                self.assertEqual(self.remote_calls, [])
                self.assertEqual(self.fixture.refs(), self.fixture.initial_refs)

    def test_foreign_repository_and_wrong_selection_fail_without_refs(self):
        self.repository["full_name"] = "foreign/repository"
        with self.assertRaises(driver.ReleaseError):
            self.publish()
        with self.assertRaises(publisher.AdapterError):
            self.publish(pipeline_id="124")
        self.assertEqual(self.remote_calls, [])
        self.assertEqual(self.fixture.refs(), self.fixture.initial_refs)

    def test_dirty_existing_releases_are_never_changed(self):
        self.publish()
        good = dict(self.release)
        refs = self.fixture.refs()
        for field, value in [("tag_name", "v2.0.0"), ("body", "foreign"), ("draft", True),
                             ("prerelease", True), ("name", "foreign"), ("id", "41"),
                             ("author", {"login": "foreign", "id": 1}),
                             ("html_url", "https://foreign.invalid")]:
            with self.subTest(field=field):
                self.release = {**good, field: value}
                with self.assertRaises(driver.ReleaseError):
                    self.publish()
                self.assertEqual(self.release[field], value)
                self.assertEqual(self.posts, 1)
                self.assertEqual(self.fixture.refs(), refs)

    def test_actual_tag_is_authoritative_and_must_be_lightweight(self):
        self.publish()
        self.release["target_commitish"] = "main"
        self.assertEqual(self.publish()["release_status"], "unchanged")
        for target in ({"type": "commit", "sha": "f" * 40},
                       {"type": "tag", "sha": self.release["target_commitish"]}):
            self.tag_override = {"ref": "refs/tags/1.0.0", "object": target}
            with self.assertRaises(driver.ReleaseError):
                self.publish()
        self.assertEqual(self.posts, 1)

    def test_output_symlink_is_refused_before_refs(self):
        self.output.symlink_to(self.fixture.root / "unrelated")
        with self.assertRaises(publisher.PublishError):
            self.publish()
        self.assertEqual(self.remote_calls, [])

    def test_cli_sanitizes_server_errors(self):
        self.repository = {"private": "fixture-only"}
        argv = ["release", "--source", str(self.fixture.source), "--selection", str(self.fixture.release),
                "--commit", self.fixture.sha, "--pipeline-id", "123", "--output", str(self.output),
                "--scratch-root", str(self.fixture.scratch)]
        with patch.object(driver.sys, "argv", argv), patch("sys.stderr", new_callable=io.StringIO) as error:
            self.assertEqual(driver.main(), 1)
        self.assertNotIn("fixture-only", error.getvalue())
        self.assertNotIn(str(self.fixture.root), error.getvalue())

    def test_native_https_git_askpass_ignores_global_helpers(self):
        # Exercise real libcurl HTTPS authentication against the local TLS
        # fixture, not the local-bare rewrite used for snapshot publication.
        helper = self.fixture.root / "askpass"
        helper.write_text('#!/bin/sh\ncase "$1" in *Username*) echo x-access-token;; '
                          '*Password*) printf "%s\\n" "$GITHUB_PUBLIC_TOKEN";; esac\n')
        helper.chmod(0o700)
        global_config = self.fixture.root / "gitconfig"
        marker = self.fixture.root / "helper-ran"
        global_config.write_text(f'[credential]\n helper = "!touch {marker}"\n')
        with patch.dict(os.environ, GIT_CONFIG_GLOBAL=str(global_config),
                        GIT_TRACE=str(self.fixture.root / "trace")):
            result = publisher.run_git(self.fixture.source, "-c", f"http.sslCAInfo={self.cert}",
                              "-c", f"http.curloptResolve=github.com:{self.server.server_port}:127.0.0.1",
                              "ls-remote", f"https://github.com:{self.server.server_port}/git/",
                              environment={"GIT_ASKPASS": str(helper), "GITHUB_PUBLIC_TOKEN": "fixture-only"},
                              allowed=(0, 128))
        self.assertEqual(result.returncode, 0, result.stderr.decode().replace("fixture-only", "REDACTED"))
        expected = "Basic " + base64.b64encode(b"x-access-token:fixture-only").decode()
        self.assertTrue(any(headers.get("Authorization") == expected for _, headers in self.observed))
        self.assertFalse(marker.exists())
        self.assertFalse((self.fixture.root / "trace").exists())


if __name__ == "__main__":
    unittest.main()
