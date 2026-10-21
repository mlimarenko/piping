"""Check the native client's daemon-namespace and source-export contract."""

import importlib.util
import io
import hashlib
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("ci_compose_native", ROOT / "scripts/ci-compose-native.py")
native = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(native)


class NativeFixtureTransportTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.binary = Path(directory.name) / "backend"
        self.binary.write_bytes(b"native fixture identity")
        self.digest = hashlib.sha256(self.binary.read_bytes()).hexdigest()
        self.native_kwargs = {"backend_binary": self.binary, "backend_sha256": self.digest,
                              "migration23_sha256": "a" * 64}
        self.binary_args = ["--backend-binary", str(self.binary), "--backend-sha256", self.digest]

    def test_client_uses_daemon_namespace_and_unix_socket_not_job_tls(self):
        with patch.object(native.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0)
            self.assertEqual(native.run_native("fixture-client:test", b"source tar", {}, **self.native_kwargs), 0)
        calls = run.call_args_list
        launch = calls[0].args[0]
        name = launch[launch.index("--name") + 1]
        self.assertEqual(launch[launch.index("--network") + 1], "host")
        self.assertEqual(launch[launch.index("--mount") + 1],
                         "type=bind,src=/var/run/docker.sock,dst=/var/run/docker.sock,readonly")
        self.assertIn("DOCKER_HOST=unix:///var/run/docker.sock", launch)
        artifact_root = "/data/artifacts/" + name
        self.assertIn("PIPING_NATIVE_SOCKET_ROOT=" + artifact_root, launch)
        self.assertLess(len((artifact_root + "/two-store-12345678/docker.sock").encode()), 108)
        self.assertEqual(launch.count("--mount"), 1)
        volume = launch[launch.index("--volume") + 1]
        self.assertTrue(volume.startswith("/data/artifacts/pwa-native-client-"))
        self.assertEqual(*volume.split(":"))
        self.assertNotIn("-v", launch)
        self.assertFalse(any("TLS" in value or "CERT" in value for value in launch))
        self.assertEqual(calls[2].args[0], ["docker", "cp", "-", name + ":/workspace"])
        self.assertEqual(calls[2].kwargs["input"], b"source tar")
        self.assertEqual(calls[3].args[0], ["docker", "cp", str(self.binary), name + ":/workspace/piping-backend"])
        execution = calls[4].args[0]
        self.assertEqual(execution, ["docker", "exec", "-i", name, "python3", "-", "--client-unittest", *native.NATIVE_TESTS])
        self.assertEqual(calls[4].kwargs["input"], Path(native.__file__).read_bytes())
        self.assertEqual(calls[-1].args[0], ["docker", "rm", "-f", name])
        cleanup = calls[-2].args[0]
        self.assertEqual(cleanup[:7], ["docker", "run", "--rm", "--pull", "never", "--network", "none"])
        self.assertIn("type=bind,src=" + artifact_root + ",dst=/cleanup", cleanup)
        self.assertEqual(cleanup[cleanup.index("--user") + 1], "0")

    def test_current_baseline_selects_only_current_tests_and_identity(self):
        with patch.object(native.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0)
            self.assertEqual(native.run_native("fixture-client:test", b"source tar", {},
                             backend_binary=self.binary, backend_sha256=self.digest,
                             baseline_source_sha="b" * 40, baseline_sha256="a" * 64), 0)
        launch = run.call_args_list[0].args[0]
        self.assertIn("PIPING_NATIVE_BASELINE_SHA256=" + "a" * 64, launch)
        self.assertIn("PIPING_NATIVE_SOURCE_SHA=" + "b" * 40, launch)
        self.assertFalse(any("MIGRATION23" in value for value in launch))
        execution = run.call_args_list[4].args[0]
        self.assertEqual(execution[execution.index("--client-unittest") + 1:], list(native.BASELINE_TESTS))
        self.assertNotIn("tests.compose.test_deploy_release_bundle.NativeHistoricalPreservationTests", execution)

    def test_current_baseline_rejects_historical_mixed_inputs_before_docker(self):
        with patch.object(native.subprocess, "run") as run, self.assertRaises(ValueError):
            native.run_native("fixture-client:test", b"source tar", {}, **self.native_kwargs,
                              baseline_source_sha="b" * 40, baseline_sha256="a" * 64)
        run.assert_not_called()

    def test_baseline_collector_binds_current_epoch_and_rejects_extra_private_fields(self):
        root = self.binary.parent / "receipts"
        root.mkdir()
        source, backend, baseline, helper = "b" * 40, self.digest, "a" * 64, "c" * 64
        migration = {"source_sha": source, "backend_sha256": backend, "baseline_sha256": baseline,
                     "fresh_baseline": True, "nonempty_replay_unchanged": True,
                     "foreign_history_refused_unchanged": True}
        flow = {"schema": "piping.compose-native-flow/v1", "helper_sha256": helper,
                "synthetic_transport_cohort": True, "native_migrator_proved": True,
                "application_source": source, "candidate_absent_before_stage": True,
                "registry_unavailable_before_stage": True, "restore_failure_rejected": True,
                "receipt_forgery_rejected": True, "replay_rejected": True,
                "configuration_database_trigger_drift_rejected": True, "env_only_baseline": True,
                "optional_proxy_absent": True, "failed_start_writers_stopped": True, "ledger_versions": 1}
        for name, value in (("baseline-migration-receipt.json", migration), ("native-flow-receipt.json", flow)):
            (root / name).write_text(json.dumps(value))
        output = self.binary.parent / "safe-baseline"
        native.collect_baseline_proof(root, output, helper, source, backend, baseline)
        public = json.loads((output / "baseline-native-proof.json").read_text())
        self.assertEqual(public["receipts"]["native-flow-receipt.json"]["ledger_versions"], 1)
        for changed in (dict(flow, ledger_versions=24), dict(flow, ledger_versions=True),
                        dict(flow, application_source=native.APPLICATION_REVISION),
                        dict(flow, private_ciphertext="never export"), dict(flow, env_only_baseline=False)):
            (root / "native-flow-receipt.json").write_text(json.dumps(changed))
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                native.collect_baseline_proof(root, self.binary.parent / "refused-output", helper, source, backend, baseline)
            self.assertFalse((self.binary.parent / "refused-output").exists())

    def test_baseline_cli_rejects_mixed_modes_before_export_or_docker(self):
        for flags in (["--fixture-working-tree"], ["--install"], ["--https"],
                      ["--download-postgres-fixture", str(self.binary.parent / "pg.tar")],
                      ["--client-unittest", "tests.ci.test_sqlx_rebaseline"],
                      ["--artifact-inputs", str(self.binary.parent)]):
            with self.subTest(flags=flags), patch.object(native.subprocess, "run") as run, \
                    patch("sys.argv", ["native", "--baseline", *flags]), \
                    patch("sys.stderr", new=io.StringIO()), self.assertRaises(SystemExit) as error:
                native.main()
            self.assertEqual(error.exception.code, 2)
            run.assert_not_called()

    def test_baseline_entrypoint_binds_its_export_and_binary_without_migration23(self):
        content = io.BytesIO()
        with tarfile.open(fileobj=content, mode="w") as bundle:
            member = tarfile.TarInfo("apps/api/migrations/0001_init.sql")
            member.size = len(b"baseline SQL")
            bundle.addfile(member, io.BytesIO(b"baseline SQL"))
        source = "b" * 40
        with patch.dict(os.environ, {}, clear=True), \
                patch.object(native, "baseline_source_archive", return_value=(content.getvalue(), source)) as archive, \
                patch.object(native, "run_native", return_value=0) as client, \
                patch("sys.argv", ["native", "--baseline", "--backend-source-sha", source,
                                   "--image", "fixture:test", *self.binary_args]):
            self.assertEqual(native.main(), 0)
        archive.assert_called_once_with(ROOT, "HEAD")
        self.assertEqual(client.call_args.kwargs["baseline_source_sha"], source)
        self.assertEqual(client.call_args.kwargs["baseline_sha256"], hashlib.sha256(b"baseline SQL").hexdigest())
        self.assertNotIn("migration23_sha256", client.call_args.kwargs)

    def test_baseline_cli_refuses_backend_from_another_source_before_docker(self):
        with patch.object(native, "baseline_source_archive", return_value=(b"tar", "b" * 40)), \
                patch.object(native.subprocess, "run") as run, \
                patch("sys.argv", ["native", "--baseline", "--backend-source-sha", "c" * 40,
                                   "--image", "fixture:test", *self.binary_args]), \
                patch("sys.stderr", new=io.StringIO()), self.assertRaises(SystemExit) as error:
            native.main()
        self.assertEqual(error.exception.code, 2)
        run.assert_not_called()

    def preservation_receipt(self):
        return {
            "schema": "piping.compose-preservation-proof/v1",
            "helper_sha256": self.digest,
            "application_source": native.APPLICATION_REVISION,
            "backend_binary_sha256": native.BACKEND_BINARY_SHA256,
            "backend_image": native.BACKEND_IMAGE,
            "prefix_sha256": native.CHECK_PREFIX_SHA256,
            "postgres_image": native.POSTGRES_FIXTURE_IMAGE_ID,
            "successful_23_empty_tail_restore_apply": True,
            "approved_objects": 136, "synthetic_relations": 8,
            "physical_layout_tables": 11, "native_check_bindings": 12,
            "native_leader_restore_and_apply": "passed",
            "preservation_sha256": "a" * 64,
            "all12_native_checks_including_nested_and": True,
            "all_preexisting_objects_exact": True,
            "all_old_column_multisets_conserved": True,
            "sequences_and_ledger_metadata_conserved": True,
            "original_check_bindings_and_layout_conserved": True,
            "approval_required_before_writer_stop": True,
            "incomplete_expired_wrong_target_approval_denied": True,
        }

    def test_artifact_inputs_use_the_same_client_and_only_the_published_scope(self):
        inputs = self.binary.parent / "carried-inputs"
        inputs.mkdir(mode=0o700)
        with patch.object(native.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0)
            self.assertEqual(native.run_native("fixture-client:test", b"source tar", {},
                artifact_inputs=inputs, **self.native_kwargs), 0)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertIn("PIPING_NATIVE_ARTIFACT_INPUTS=/workspace/artifact-inputs", commands[0])
        self.assertTrue(any(command[:3] == ["docker", "cp", str(inputs)] for command in commands))
        execution = next(command for command in commands if "--client-unittest" in command)
        self.assertEqual(execution[-1], "tests.compose.test_deploy_release_bundle.NativePublishedArtifactTests")

    def test_real_unittest_projection_keeps_messages_parameters_and_fd_output_private(self):
        root = self.binary.parent
        (root / "tests").mkdir()
        (root / "tests/__init__.py").write_text("")
        (root / "tests/fatal.py").write_text('raise SystemExit("synthetic-projection-secret")\n')
        (root / "tests/projection.py").write_text('''import unittest, os
class Case(unittest.TestCase):
    def id(self): return "synthetic-projection-secret"
    def test_subtests(self):
        os.write(1, b"synthetic-projection-secret")
        os.write(2, b"synthetic-projection-secret")
        for i in range(20):
            with self.subTest(secret="synthetic-projection-secret", i=i):
                self.fail("synthetic-projection-secret")
    def test_error(self): raise ValueError("synthetic-projection-secret")
class Fixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls): raise RuntimeError("synthetic-projection-secret")
    def test_not_run(self): pass
class Passing(unittest.TestCase):
    def test_pass(self): pass
''')
        for selector, expected in (("tests.projection", 1), ("tests.projection.Passing", 0),
                                   ("tests.projection.Fixture", 1), ("tests.fatal", 1)):
            private = root / ("private-" + selector)
            private.mkdir(mode=0o700)
            result = subprocess.run([sys.executable, "-", "--client-unittest", selector],
                input=Path(native.__file__).read_bytes(), cwd=root,
                env={**os.environ, "PIPING_NATIVE_ARTIFACT_ROOT": str(private)}, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, expected)
            self.assertEqual(result.stderr, b"")
            self.assertNotIn(b"synthetic-projection-secret", result.stdout)
            summary = json.loads(result.stdout)
            self.assertEqual(summary["exit"], expected)
            self.assertLess(len(result.stdout), 4096)
            if selector == "tests.projection":
                self.assertEqual((summary["failures"], summary["errors"]), (20, 2))
                self.assertEqual(len(summary["events"]), 12)
                self.assertTrue(all(e["test"] in ("tests.projection.Case.test_error", "tests.projection.Case.test_subtests")
                                    for e in summary["events"]))
                self.assertTrue(all(e["source_file"] == "tests/projection.py" for e in summary["events"]))
                self.assertEqual({e["source_line"] for e in summary["events"]}, {9, 10})
                self.assertIn("synthetic-projection-secret", (private / "client-output.log").read_text())
            elif selector.endswith("Fixture"):
                self.assertEqual(summary["events"], [{"test": "tests.projection.Fixture.setUpClass", "exception": "RuntimeError",
                                                     "source_file": "tests/projection.py", "source_line": 13}])
                self.assertEqual(summary["errors"], 1)
            elif selector.endswith("fatal"):
                self.assertEqual(summary["events"], [{"test": "unittest.runner", "exception": "SystemExit"}])
            self.assertEqual((private / "client-output.log").stat().st_mode & 0o777, 0o600)

    def test_failure_location_ignores_foreign_and_untrusted_traceback_paths(self):
        root = self.binary.parent
        (root / "tests").mkdir()
        (root / "tests/__init__.py").write_text("")
        (root / "tests/location.py").write_text('''import unittest
class Case(unittest.TestCase):
    def test_foreign(self):
        exec(compile('raise AssertionError("synthetic-projection-secret")',
                     '/private/synthetic-projection-secret.py', 'exec'))
    def test_injected(self):
        exec(compile('raise AssertionError("synthetic-projection-secret")',
                     'tests/invalid\\nsynthetic-projection-secret.py', 'exec'))
    def test_helper(self):
        self.command()
    def command(self):
        self.fail("synthetic-projection-secret")
    def test_nonexistent(self):
        exec(compile('def test_nonexistent(): raise AssertionError("synthetic-projection-secret")\\ntest_nonexistent()',
                     'tests/synthetic_projection_secret.py', 'exec'))
class Fixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        exec(compile('raise RuntimeError("synthetic-projection-secret")',
                     'tests/synthetic_projection_secret.py', 'exec'))
    def test_never_run(self): pass
''')
        private = root / "private"
        private.mkdir(mode=0o700)
        result = subprocess.run([sys.executable, "-", "--client-unittest", "tests.location"],
            input=Path(native.__file__).read_bytes(), cwd=root,
            env={**os.environ, "PIPING_NATIVE_ARTIFACT_ROOT": str(private)}, capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr, b"")
        self.assertNotIn(b"synthetic-projection-secret", result.stdout)
        self.assertNotIn(b"synthetic_projection_secret", result.stdout)
        summary = json.loads(result.stdout)
        self.assertEqual((summary["tests_run"], summary["failures"], summary["errors"]), (4, 4, 1))
        self.assertEqual({(e["source_file"], e["source_line"]) for e in summary["events"]},
                         {("tests/location.py", line) for line in (4, 7, 10, 14, 19)})
        self.assertTrue(all(set(e) == {"test", "exception", "source_file", "source_line"}
                            for e in summary["events"]))
        self.assertEqual((private / "client-output.log").stat().st_mode & 0o777, 0o600)

    def test_published_artifact_collection_is_exact_and_does_not_export_private_inputs(self):
        root = self.binary.parent / "private-artifact-proof"
        root.mkdir(mode=0o700)
        receipt = {
            "application_source": native.PRODUCER_APPLICATION_REVISION, "backend_binary_sha256": native.PRODUCER_BINARY_SHA256,
            "published_ref": native.PRODUCER_IMAGES["backend"], "postgres_image": native.POSTGRES_FIXTURE_IMAGE_ID,
            "helper_sha256": self.digest,
            "scope": "native archive/migration acceptance; no product startup or historical-target approval",
            "cohorts": [{"installed": v, "dump_sha256": "a" * 64,
                         "native_restore_migration_conservation": "passed"} for v in (23, 24)],
        }
        path = root / "published-restore-receipt.json"
        path.write_text(json.dumps(receipt))
        (root / "private-env").write_text("not public")
        output = self.binary.parent / "safe-artifact-proof"
        native.collect_artifact_proof(root, output, self.digest)
        self.assertEqual([p.name for p in output.iterdir()], [path.name])
        for index, update in enumerate(({"application_source": "a" * 40}, {"cohorts": receipt["cohorts"][:1]},
                                        {"private_key": "not public"}, {"helper_sha256": "b" * 64})):
            path.write_text(json.dumps({**receipt, **update}))
            target = self.binary.parent / ("bad-artifact-" + str(index))
            with self.assertRaises(ValueError):
                native.collect_artifact_proof(root, target, self.digest)
            self.assertFalse(target.exists())

    def test_public_collection_requires_complete_source_bound_preservation(self):
        root = self.binary.parent / "native-private"
        root.mkdir()
        preservation = root / "historical-preservation-public.json"
        for name in native.PUBLIC_RECEIPTS - {preservation.name}:
            (root / name).write_text('{}')
        (root / 'private-check-capture.json').write_text('{"private":true}')
        preservation.write_text(json.dumps(self.preservation_receipt()))
        output = self.binary.parent / 'good-public'
        native.collect_public_proof(root, output, self.digest)
        native.validate_public_proof(output, self.digest)
        self.assertFalse((output / 'private-check-capture.json').exists())
        for number, change in enumerate((
            {"all12_native_checks_including_nested_and": False},
            {"helper_sha256": "b" * 64},
            {"application_source": "b" * 40},
            {"application_source": "96c9833e6a4cfdee0eff9c206fcd2cbee9e8dd53"},
            {"application_source": "8f812de3adc4b62abbfa3400d9539b25cdb7058a"},
            {"application_source": "75b9733bc7e31fdfc9a5f51e1f00b6b4fac761dc"},
            {"application_source": "9e457c484e4b032bd1df0c57b72e937fc8c29902"},
            {"application_source": "70254dc18cd36e8717de88284ba7456a12f50742"},
            {"application_source": "ccb216481bcbc4d54315205269753e5d9d276d29"},
            {"backend_binary_sha256": "b" * 64},
            {"backend_image": "other"},
            {"backend_image": "gitlab.piping.space:4567/piping-space/products/piping/piping.app/backend@sha256:bdbef628fce4f01f73176d57712c302da469bc00296f82c86bbdfdbe55b910c7"},
            {"backend_image": "gitlab.piping.space:4567/piping-space/products/piping/piping.app/backend@sha256:2cfcba47f3856209dc9024f1a7683a479cabf0b78cb62e5fc45cdd257dcc143d"},
            {"backend_image": "gitlab.piping.space:4567/piping-space/products/piping/piping.app/backend@sha256:53792c799d553ef586ba5a2165fea22e1a3426d91a8ad3f0d11fe0b57739c39c"},
            {"backend_image": "gitlab.piping.space:4567/piping-space/products/piping/piping.app/backend@sha256:96445ef8ab080765418b1a2c7fd68e6f12fb8bbb6e31bebfc176b4d536e50865"},
            {"backend_image": "gitlab.piping.space:4567/piping-space/products/piping/piping.app/backend@sha256:50b9d7c4fad39e746891221f7a570e97eaee050c0427e517945e32d5517f3c0d"},
            {"backend_image": "gitlab.piping.space:4567/piping-space/products/piping/piping.app/backend@sha256:047bfd3b849a67516ccce90fa56ad9f5b7001952b38bed4c55b4ef3a78133bd5"},
            {"prefix_sha256": "b" * 64},
            {"postgres_image": "other"},
            {"successful_23_empty_tail_restore_apply": False},
            {"native_check_bindings": 11},
            {"private_check_capture": {"oid": 123}},
        )):
            value = dict(self.preservation_receipt(), **change)
            preservation.write_text(json.dumps(value))
            bad_output = self.binary.parent / ('rejected-' + str(number))
            with self.assertRaises(ValueError):
                native.collect_public_proof(root, bad_output, self.digest)
            self.assertFalse(bad_output.exists())
        for key in ("helper_sha256", "all_preexisting_objects_exact"):
            value = self.preservation_receipt()
            del value[key]
            preservation.write_text(json.dumps(value))
            with self.assertRaises(ValueError):
                native.collect_public_proof(root, self.binary.parent / 'missing-public', self.digest)

    def test_export_validation_rejects_tampering_even_with_recomputed_index(self):
        root = self.binary.parent / 'native-private'
        root.mkdir()
        for name in native.PUBLIC_RECEIPTS:
            (root / name).write_text(json.dumps(self.preservation_receipt())
                                     if name == 'historical-preservation-public.json' else '{}')
        output = self.binary.parent / 'public'
        native.collect_public_proof(root, output, self.digest)
        index_path = output / 'index.json'
        index = json.loads(index_path.read_text())
        name = next(name for name in index['files_sha256'] if name.endswith('historical-preservation-public.json'))
        value = json.loads((output / name).read_text())
        value['all_old_column_multisets_conserved'] = False
        (output / name).write_text(json.dumps(value))
        index['files_sha256'][name] = hashlib.sha256((output / name).read_bytes()).hexdigest()
        index_path.write_text(json.dumps(index))
        with self.assertRaises(ValueError):
            native.validate_public_proof(output, self.digest)

    def test_real_exported_producer_identity_is_accepted_by_collector(self):
        workspace = self.binary.parent / 'workspace'
        workspace.mkdir()
        archive = native.source_archive(ROOT, 'HEAD', False)
        with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
            bundle.extractall(workspace, filter='data')
        producer = subprocess.run(
            [os.sys.executable, '-I', '-c',
             "import runpy,sys; print(runpy.run_path(sys.argv[1])['helper_identity']())",
             str(workspace / 'scripts/deploy_release_bundle.py')],
            check=True, capture_output=True, text=True, timeout=30,
        ).stdout.strip()
        value = self.preservation_receipt()
        value['helper_sha256'] = producer
        receipts = self.binary.parent / 'native-receipts'
        receipts.mkdir()
        for name in native.PUBLIC_RECEIPTS:
            (receipts / name).write_text(json.dumps(value) if name == 'historical-preservation-public.json' else '{}')
        output = self.binary.parent / 'collected-proof'
        native.collect_exported_public_proof(workspace, receipts, output)
        native.validate_public_proof(output, producer)
        metadata = json.loads((workspace / native.EXPORT_METADATA).read_text())
        self.assertNotEqual(producer, metadata['files_sha256']['scripts/deploy_release_bundle.py'])
        for name in ('scripts/deploy_release_bundle.py', 'scripts/ci/compose_delivery_contract.py',
                     'scripts/ci/apply_release_bundle.py', 'scripts/ci/postgres_migration_contract.py'):
            path = workspace / name
            original = path.read_bytes()
            path.write_bytes(original + b'\n# untrusted changed component\n')
            with self.assertRaisesRegex(ValueError, 'component hash mismatch'):
                native.exported_helper_identity(workspace)
            path.write_bytes(original)

    def test_private_fixture_download_rejects_size_hash_and_redirect(self):
        content = b"small transport fixture"
        target = self.binary.parent / "postgres.tar"
        class Response(io.BytesIO):
            status = 200
            headers = {}
        class Opener:
            def open(self, request, timeout):
                self.request = request
                return Response(content)
        opener = Opener()
        with patch.object(native, "POSTGRES_FIXTURE_SIZE", len(content)), \
                patch.object(native, "POSTGRES_FIXTURE_SHA256", hashlib.sha256(content).hexdigest()), \
                patch.object(native.urllib.request, "build_opener", return_value=opener), \
                patch.object(native, "verify_postgres_fixture", return_value={"verified": True}) as verify:
            self.assertEqual(native.download_postgres_fixture(target, {"CI_JOB_TOKEN": "synthetic-test-value"}), {"verified": True})
            verify.assert_called_once_with(target)
            self.assertEqual(target.read_bytes(), content)
            self.assertEqual(opener.request.get_header("Job-token"), "synthetic-test-value")
            self.assertNotIn("synthetic-test-value", opener.request.full_url)
            with self.assertRaises(FileExistsError):
                native.download_postgres_fixture(target, {"CI_JOB_TOKEN": "synthetic-test-value"})
        for number, altered in enumerate((content[:-1], content + b"!", b"X" * len(content))):
            bad = self.binary.parent / ("bad-postgres-" + str(number))
            with patch.object(native, "POSTGRES_FIXTURE_SIZE", len(content)), \
                    patch.object(native, "POSTGRES_FIXTURE_SHA256", hashlib.sha256(content).hexdigest()), \
                    patch.object(native.urllib.request, "build_opener") as builder:
                response = Response(altered)
                builder.return_value.open.return_value = response
                with self.assertRaises(ValueError):
                    native.download_postgres_fixture(bad, {"CI_JOB_TOKEN": "synthetic-test-value"})
        with patch.object(native.urllib.request, "build_opener") as builder:
            builder.return_value.open.return_value = Response(content)
            target2 = self.binary.parent / "redirect.tar"
            with self.assertRaises(ValueError):
                native.download_postgres_fixture(target2, {})
            builder.assert_not_called()
        with patch.object(native.urllib.request, "build_opener") as builder:
            builder.return_value.open.side_effect = lambda *a, **k: builder.call_args.args[0].redirect_request(None, None, 302, '', {}, 'https://untrusted.invalid')
            with self.assertRaises(ValueError):
                native.download_postgres_fixture(self.binary.parent / "redirect.tar", {"CI_JOB_TOKEN": "synthetic-test-value"})

    def test_native_failure_is_propagated_and_client_is_removed(self):
        with patch.object(native.subprocess, "run") as run:
            run.side_effect = [subprocess.CompletedProcess([], 0)] * 4 + [
                subprocess.CompletedProcess([], 1), subprocess.CompletedProcess([], 0)]
            self.assertEqual(native.run_native("fixture-client:test", b"tar", {}, **self.native_kwargs), 1)
        self.assertEqual(run.call_args_list[-1].args[0][:3], ["docker", "rm", "-f"])

    def test_public_receipts_are_collected_only_after_native_success(self):
        output = self.binary.parent / "public-proof"
        with patch.object(native.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0)
            self.assertEqual(native.run_native("fixture-client:test", b"tar", {}, proof_directory=output,
                                               **self.native_kwargs), 0)
        self.assertTrue(output.is_dir())
        commands = [call.args[0] for call in run.call_args_list]
        copies = [command for command in commands if command[:2] == ["docker", "cp"]]
        self.assertTrue(any("public-proof/." in command[2] for command in copies))
        with self.assertRaises(ValueError):
            native.run_native("fixture-client:test", b"tar", {}, proof_directory=output, **self.native_kwargs)
        failed_output = self.binary.parent / "failed-public-proof"
        with patch.object(native.subprocess, "run") as run:
            run.side_effect = [subprocess.CompletedProcess([], 0)] * 4 + [
                subprocess.CompletedProcess([], 1), subprocess.CompletedProcess([], 0)]
            self.assertEqual(native.run_native("fixture-client:test", b"tar", {}, proof_directory=failed_output,
                                               **self.native_kwargs), 1)
        self.assertFalse(failed_output.exists())

    def test_copy_failure_still_removes_client(self):
        with patch.object(native.subprocess, "run") as run:
            run.side_effect = [subprocess.CompletedProcess([], 0)] * 2 + [
                subprocess.CalledProcessError(1, "docker cp"), subprocess.CompletedProcess([], 0)]
            with self.assertRaises(subprocess.CalledProcessError):
                native.run_native("fixture-client:test", b"tar", {}, **self.native_kwargs)
        self.assertEqual(run.call_args_list[-1].args[0][:3], ["docker", "rm", "-f"])

    def test_https_uses_same_transport_without_backend_or_private_identity(self):
        for status in (0, 1):
            with self.subTest(status=status), patch.object(native.subprocess, "run") as run:
                run.side_effect = [subprocess.CompletedProcess([], 0)] * 3 + [
                    subprocess.CompletedProcess([], status),
                    subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 0)]
                self.assertEqual(native.run_https("fixture-client:test", b"committed tar", {}), status)
                calls = run.call_args_list
                launch = calls[0].args[0]
                name = launch[launch.index("--name") + 1]
                root = "/data/artifacts/" + name
                self.assertIn("PIPING_TEST_ARTIFACTS=" + root, launch)
                self.assertEqual(launch[launch.index("--network") + 1], "host")
                self.assertFalse(any("BACKEND" in arg or "MIGRATION" in arg or "REGISTRY" in arg for arg in launch))
                self.assertEqual(calls[2].kwargs["input"], b"committed tar")
                execution = calls[3].args[0]
                self.assertEqual(execution[6:], ["--client-unittest", *native.HTTPS_TESTS])
                self.assertEqual(calls[3].kwargs["timeout"], 180)
                self.assertIn("type=bind,src=" + root + ",dst=/cleanup", calls[4].args[0])
                self.assertEqual(calls[5].args[0], ["docker", "rm", "-f", name])

    def test_https_deadline_still_cleans_private_artifacts_and_client(self):
        with patch.object(native.subprocess, "run") as run:
            run.side_effect = [subprocess.CompletedProcess([], 0)] * 3 + [
                subprocess.TimeoutExpired("native HTTPS", 180),
                subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 0)]
            with self.assertRaises(subprocess.TimeoutExpired):
                native.run_https("fixture-client:test", b"tar", {})
        self.assertEqual(run.call_args_list[-2].args[0][:3], ["docker", "run", "--rm"])
        self.assertEqual(run.call_args_list[-1].args[0][:3], ["docker", "rm", "-f"])

    def test_install_uses_shared_client_and_requires_an_immutable_alternate(self):
        for alternate in ("", "frontend:mutable", "bad@sha256:not-a-digest"):
            with self.subTest(alternate=alternate), patch.object(native.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    native.run_https("fixture-client:test", b"tar", {
                        "PIPING_INSTALL_TEST_FRONTEND_IMAGE": alternate}, install=True)
                run.assert_not_called()
        alternate = "registry.example/frontend@sha256:" + "a" * 64
        with patch.object(native.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0)
            self.assertEqual(native.run_https("fixture-client:test", b"tar", {
                "PIPING_INSTALL_TEST_FRONTEND_IMAGE": alternate}, install=True), 0)
        calls = run.call_args_list
        self.assertIn("PIPING_INSTALL_TEST_FRONTEND_IMAGE=" + alternate, calls[0].args[0])
        self.assertIn("tests.compose.test_install_lifecycle", calls[3].args[0])
        self.assertEqual(calls[3].kwargs["timeout"], 900)
        self.assertEqual(calls[-1].args[0][:3], ["docker", "rm", "-f"])

    def test_https_missing_docker_is_failure_not_skip(self):
        with patch.object(native.subprocess, "run", side_effect=FileNotFoundError("docker")):
            with self.assertRaises(FileNotFoundError):
                native.run_https("fixture-client:test", b"tar", {})

    def test_https_archive_uses_committed_bytes_not_dirty_or_private_files(self):
        archive = native.https_source_archive(ROOT, "HEAD")
        with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
            self.assertIn("tests/compose/test_install_https.py", bundle.getnames())
            self.assertFalse(any(name in (".env", ".images.env") or "privkey" in name for name in bundle.getnames()))
            installer = "install.sh"
            committed = subprocess.run(["git", "show", "HEAD:" + installer], cwd=ROOT,
                                       capture_output=True, check=True, timeout=30).stdout
            self.assertEqual(bundle.extractfile(installer).read(), committed)
        with patch.object(native, "ROOT", self.binary.parent):
            with self.assertRaises(subprocess.CalledProcessError):
                native.https_source_archive(native.ROOT, "HEAD")

    def test_install_archive_keeps_current_launcher_and_the_actual_old_application_helper(self):
        archive = native.https_source_archive(ROOT, "HEAD", install=True)
        with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
            metadata = json.load(bundle.extractfile(native.EXPORT_METADATA))
            self.assertEqual(metadata["scope"], "current-launcher/pinned-prebaseline24-installation")
            self.assertEqual(metadata["application_commit"], native.ARTIFACT_HELPER_REVISION)
            current = subprocess.run(["git", "show", "HEAD:install.sh"], cwd=ROOT,
                                     capture_output=True, check=True, timeout=30).stdout
            self.assertEqual(bundle.extractfile("install.sh").read(), current)
            for name in ("env/images.env", "scripts/deploy_release_bundle.py",
                         "scripts/ci/apply_release_bundle.py", "scripts/ci/postgres_migration_contract.py",
                         "scripts/ci/compose_delivery_contract.py"):
                expected = subprocess.run(["git", "show", native.ARTIFACT_HELPER_REVISION + ":" + name],
                                          cwd=ROOT, capture_output=True, check=True, timeout=30).stdout
                observed = bundle.extractfile(name).read()
                self.assertEqual(observed, expected)
                self.assertEqual(metadata["files_sha256"][name], hashlib.sha256(observed).hexdigest())
            # Execute the exported launcher's payload declaration. Its current
            # dependencies must be usable alongside the pinned old SQL owner.
            from scripts.public_release import payload_paths

            with tempfile.TemporaryDirectory(prefix="piping-install-export-") as temporary:
                exported = Path(temporary)
                for member in bundle.getmembers():
                    target = exported / member.name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(bundle.extractfile(member).read())
                for name in payload_paths(exported):
                    self.assertTrue((exported / name).is_file(), name)

    def test_https_profiles_preserve_all_committed_nginx_inputs(self):
        tracked = subprocess.run(
            ["git", "ls-tree", "-r", "--name-only", "HEAD", "--", "docker/nginx"],
            cwd=ROOT, capture_output=True, check=True, timeout=30,
        ).stdout.decode().splitlines()
        self.assertTrue(tracked)
        expected = {
            name: subprocess.run(["git", "show", "HEAD:" + name], cwd=ROOT,
                                 capture_output=True, check=True, timeout=30).stdout
            for name in tracked
        }
        for install in (False, True):
            with self.subTest(install=install):
                archive = native.https_source_archive(ROOT, "HEAD", install=install)
                with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
                    for name, content in expected.items():
                        member = bundle.extractfile(name)
                        self.assertIsNotNone(member, name)
                        self.assertEqual(member.read(), content, name)

    def test_export_is_allowlisted_and_local_overlay_is_exact(self):
        for dirty in (False, True):
            with self.subTest(dirty=dirty):
                archive = native.source_archive(ROOT, "HEAD", dirty)
                with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
                    names = [m.name for m in bundle]
                    self.assertEqual(len(names), len(set(names)))
                    for member in bundle:
                        self.assertFalse(member.issym() or member.islnk())
                        if member.isfile():
                            self.assertTrue(member.name in native.SOURCE_PATHS
                                            or member.name == native.EXPORT_METADATA
                                            or member.name.startswith("apps/api/migrations/"), member.name)
                    content = bundle.extractfile(native.FIXTURE).read()
                expected = (ROOT / native.FIXTURE).read_bytes() if dirty else subprocess.run(
                    ["git", "show", "HEAD:" + native.FIXTURE], cwd=ROOT,
                    capture_output=True, check=True, timeout=30,
                ).stdout
                self.assertEqual(content, expected)
                with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
                    sql = "apps/api/migrations/0023_secret_field_lifecycle_management.sql"
                    actual_sql = bundle.extractfile(sql).read()
                published_sql = subprocess.run(["git", "show", native.APPLICATION_REVISION + ":" + sql],
                                               cwd=ROOT, capture_output=True, check=True, timeout=30).stdout
                self.assertEqual(actual_sql, published_sql)
                with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
                    metadata = json.loads(bundle.extractfile(native.EXPORT_METADATA).read())
                    self.assertEqual(metadata["application_commit"], native.APPLICATION_REVISION)
                    self.assertEqual(metadata["dirty_helper_overlay"], dirty)
                    for name, expected_hash in metadata["files_sha256"].items():
                        self.assertEqual(hashlib.sha256(bundle.extractfile(name).read()).hexdigest(), expected_hash)

    def test_source_export_needs_no_docker_registry_or_executable_and_is_exclusive(self):
        target = self.binary.parent / "fixture.tar"
        with patch("sys.argv", ["ci-compose-native.py", "--export-source", str(target)]):
            self.assertEqual(native.main(), 0)
            with self.assertRaises(FileExistsError):
                native.main()
        with tarfile.open(target) as archive:
            self.assertIn(native.EXPORT_METADATA, archive.getnames())

    def test_current_source_export_binds_application_and_helper_to_one_commit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.run(["git", "-C", str(root), *args], check=True,
                                      capture_output=True).stdout.decode().strip()
            git("init", "-b", "fixture")
            git("config", "user.email", "fixture@example.invalid")
            git("config", "user.name", "Export Fixture")
            for name, data in (("apps/api/migrations/0001_init.sql", b"committed baseline\n"),
                               ("apps/api/config/default.toml", b"committed config\n"),
                               ("scripts/rebaseline_sqlx.py", b"committed maintenance\n"),
                               ("scripts/ci-compose-native.py", b"committed selector\n")):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            git("add", "--all")
            git("commit", "-m", "current fixture")
            expected = git("rev-parse", "HEAD")
            (root / "apps/api/migrations/0001_init.sql").write_bytes(b"dirty baseline\n")
            (root / ".env").write_bytes(b"private fixture\n")
            archive, source = native.baseline_source_archive(root, "HEAD")
            self.assertEqual(source, expected)
            with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
                metadata = json.load(bundle.extractfile(native.EXPORT_METADATA))
                self.assertEqual(metadata["helper_commit"], expected)
                self.assertEqual(metadata["application_commit"], expected)
                self.assertFalse(metadata["dirty_helper_overlay"])
                self.assertEqual(bundle.extractfile("apps/api/migrations/0001_init.sql").read(), b"committed baseline\n")
                self.assertNotIn(".env", bundle.getnames())
                self.assertNotIn("apps/api/migrations/0023_secret_field_lifecycle_management.sql", bundle.getnames())

    def test_historical_exports_match_the_prebaseline_exporter_byte_for_byte(self):
        old_source = subprocess.run(["git", "show", native.ARTIFACT_HELPER_REVISION + ":scripts/ci-compose-native.py"],
                                    cwd=ROOT, capture_output=True, check=True, timeout=30).stdout
        old_file = self.binary.parent / "prebaseline_native.py"
        old_file.write_bytes(old_source)
        original = runpy.run_path(str(old_file))
        for helper, application in ((native.HISTORICAL_HELPER_REVISION, native.APPLICATION_REVISION),
                                    (native.ARTIFACT_HELPER_REVISION, native.PRODUCER_APPLICATION_REVISION)):
            with self.subTest(helper=helper):
                expected = original["source_archive"](ROOT, helper, False, application_revision=application)
                actual = native.source_archive(ROOT, helper, False, application_revision=application)
                self.assertEqual(actual, expected)

    def test_committed_focused_contract_is_exported_without_private_capture(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.run(["git", "-C", str(root), *args], check=True,
                                      capture_output=True).stdout.decode().strip()
            def write(name, content):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
            git("init", "-b", "fixture")
            git("config", "user.email", "fixture@example.invalid")
            git("config", "user.name", "Export Fixture")
            write("apps/api/migrations/0023_secret_field_lifecycle_management.sql", b"published SQL\n")
            write("apps/api/migrations/compose-deployment.json", b"{}\n")
            write("apps/api/config/default.toml", b"published config\n")
            git("add", "--all")
            git("commit", "-m", "application fixture")
            application = git("rev-parse", "HEAD")
            focused = "tests/ci/test_postgres_migration_contract.py"
            shared = "scripts/ci/postgres_migration_contract.py"
            write(focused, b"committed focused proof\n")
            write(shared, b"committed shared contract\n")
            write("tests/ci/unreviewed_schema_contract.py", b"unreviewed sibling\n")
            write("private/original-check-bindings.json", b"private original capture\n")
            git("add", "--all")
            git("commit", "-m", "helper fixture")
            helper = git("rev-parse", "HEAD")
            write(focused, b"uncommitted focused overlay\n")
            write("apps/api/migrations/0023_secret_field_lifecycle_management.sql", b"uncommitted product SQL\n")
            archive = native.source_archive(root, helper, False, application_revision=application)
            with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
                self.assertEqual(bundle.extractfile(focused).read(), b"committed focused proof\n")
                self.assertEqual(bundle.extractfile(shared).read(), b"committed shared contract\n")
                self.assertEqual(bundle.extractfile("apps/api/migrations/0023_secret_field_lifecycle_management.sql").read(), b"published SQL\n")
                self.assertNotIn("private/original-check-bindings.json", bundle.getnames())
                self.assertNotIn("tests/ci/unreviewed_schema_contract.py", bundle.getnames())
                metadata = json.load(bundle.extractfile(native.EXPORT_METADATA))
                self.assertEqual(metadata["helper_commit"], helper)
                self.assertEqual(metadata["application_commit"], application)
                self.assertFalse(metadata["dirty_helper_overlay"])
                self.assertEqual(set(bundle.getnames()), set(metadata["files_sha256"]) | {native.EXPORT_METADATA})
                for name, digest in metadata["files_sha256"].items():
                    self.assertEqual(hashlib.sha256(bundle.extractfile(name).read()).hexdigest(), digest)

    def test_ci_exports_exact_commit_and_refuses_dirty_fixture(self):
        content = io.BytesIO()
        with tarfile.open(fileobj=content, mode="w") as bundle:
            migration = tarfile.TarInfo("apps/api/migrations/0023_secret_field_lifecycle_management.sql")
            migration.size = 6
            bundle.addfile(migration, io.BytesIO(b"SQL23;"))
        environment = {"CI": "true", "CI_COMMIT_SHA": "c" * 40,
                       "CI_TOOLS_IMAGE": "registry.example.test/tools/client:test",
                       "CI_REGISTRY": "registry.example.test", "CI_REGISTRY_USER": "gitlab-ci-token",
                       "CI_REGISTRY_PASSWORD": "synthetic-test-value"}  # pragma: allowlist secret - synthetic fixture, not a credential
        with patch.dict(os.environ, environment, clear=True), \
                patch.object(native, "source_archive", return_value=content.getvalue()) as archive, \
                patch.object(native, "run_native", return_value=0) as client, \
                patch.object(native.subprocess, "run") as login, \
                patch("sys.argv", ["ci-compose-native.py", *self.binary_args]):
            self.assertEqual(native.main(), 0)
            archive.assert_called_once_with(ROOT, native.HISTORICAL_HELPER_REVISION, False)
            self.assertEqual(login.call_count, 1)
            self.assertEqual(login.call_args.args[0], ["docker", "login", "--username", "gitlab-ci-token",
                                                     "--password-stdin", "registry.example.test"])
            self.assertEqual(login.call_args.kwargs["input"], b"synthetic-test-value")
            self.assertNotIn("synthetic-test-value", login.call_args.args[0])
            self.assertEqual(login.call_args.kwargs["env"]["DOCKER_CONFIG"], client.call_args.args[2]["DOCKER_CONFIG"])
            self.assertFalse(Path(client.call_args.args[2]["DOCKER_CONFIG"]).exists())
        with patch.dict(os.environ, environment, clear=True), \
                patch("sys.argv", ["ci-compose-native.py", "--fixture-working-tree"]), \
                patch("sys.stderr", new=io.StringIO()):
            with self.assertRaises(SystemExit) as rejected:
                native.main()
            self.assertEqual(rejected.exception.code, 2)

    def test_carried_artifact_entrypoint_keeps_its_prebaseline_helper_and_historical_default(self):
        inputs = self.binary.parent / "entrypoint-inputs"
        inputs.mkdir(mode=0o700)
        content = io.BytesIO()
        with tarfile.open(fileobj=content, mode="w") as bundle:
            migration = tarfile.TarInfo("apps/api/migrations/0023_secret_field_lifecycle_management.sql")
            migration.size = 6
            bundle.addfile(migration, io.BytesIO(b"SQL23;"))
        for carried in (False, True):
            with self.subTest(carried=carried), patch.dict(os.environ, {}, clear=True), \
                    patch.object(native, "source_archive", return_value=content.getvalue()) as archive, \
                    patch.object(native, "run_native", return_value=0) as client, \
                    patch("sys.argv", ["ci-compose-native.py", "--image", "fixture-client:test", *self.binary_args,
                                       *(["--artifact-inputs", str(inputs)] if carried else [])]):
                self.assertEqual(native.main(), 0)
                expected = {"application_revision": native.PRODUCER_APPLICATION_REVISION} if carried else {}
                archive.assert_called_once_with(ROOT, native.ARTIFACT_HELPER_REVISION if carried else native.HISTORICAL_HELPER_REVISION, False, **expected)
                self.assertEqual(client.call_args.kwargs.get("artifact_inputs"), inputs if carried else None)

    def test_ci_requires_its_registry_identity_before_starting_client(self):
        cases = [
            {"CI": "true", "CI_TOOLS_IMAGE": "registry.example.test/tools/client:test"},
            {"CI": "true", "CI_TOOLS_IMAGE": "untrusted.example.test/tools/client:test",
             "CI_REGISTRY": "registry.example.test", "CI_REGISTRY_USER": "gitlab-ci-token",
             "CI_REGISTRY_PASSWORD": "synthetic-test-value"},  # pragma: allowlist secret - synthetic fixture, not a credential
        ]
        for environment in cases:
            with self.subTest(environment=environment), patch.dict(os.environ, environment, clear=True), \
                    patch.object(native, "run_native") as client, \
                    patch.object(native, "source_archive") as archive, \
                    patch("sys.argv", ["ci-compose-native.py", *self.binary_args]), patch("sys.stderr", new=io.StringIO()):
                with self.assertRaises(SystemExit) as rejected:
                    native.main()
                self.assertEqual(rejected.exception.code, 2)
                client.assert_not_called()
                archive.assert_not_called()

    def test_binary_copy_failure_removes_client_before_native_execution(self):
        with patch.object(native.subprocess, "run") as run:
            run.side_effect = [subprocess.CompletedProcess([], 0)] * 3 + [
                subprocess.CalledProcessError(1, "docker cp"), subprocess.CompletedProcess([], 0)]
            with self.assertRaises(subprocess.CalledProcessError):
                native.run_native("fixture-client:test", b"tar", {}, **self.native_kwargs)
        self.assertEqual(run.call_args_list[-1].args[0][:3], ["docker", "rm", "-f"])


if __name__ == "__main__":
    unittest.main()
