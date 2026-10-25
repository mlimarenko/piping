"""Secondary stdlib boundary checks; native engine coverage lives in tests/compose."""

from __future__ import annotations

import argparse
import copy
from contextlib import nullcontext
from datetime import datetime, timedelta, timezone
import hashlib
import io
import json
import os
import socket
import stat
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import deploy_release_bundle as deploy
from scripts.ci.compose_delivery_contract import canonical, save_authority, save_identity, verify_manifest
from scripts.ci.compose_delivery_contract import package_binding, preservation_manifest, PRESERVATION_CONTEXT


def migration_archive(extra=None, *, omit=None, count=19):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        directory = tarfile.TarInfo("migrations")
        directory.type = tarfile.DIRTYPE
        archive.addfile(directory)
        for version in range(1, count + 1):
            if version == omit:
                continue
            content = f"-- migration {version}\nSELECT {version};\n".encode()
            item = tarfile.TarInfo(f"migrations/{version:04d}_test.sql")
            item.size = len(content)
            archive.addfile(item, io.BytesIO(content))
        if extra:
            archive.addfile(extra, io.BytesIO(b"x" * extra.size) if extra.isfile() else None)
    return output.getvalue()


class ComposeDeploymentBoundaryTests(unittest.TestCase):
    def test_only_transitioned_baseline_history_is_accepted(self):
        deployment = deploy.Deployment(self.args)
        migrations = {"1": hashlib.sha384(b"baseline").hexdigest()}
        history = [{"version": "1", "checksum": migrations["1"], "success": True,
                    "description": "init", "installed_on": "2026-10-21T00:00:00Z", "execution_time": 1}]
        deployment.plan = {"migrations": migrations, "contract": {"installed_prefix": 1},
                           "disabled_schedule": None, "installed_history": history}
        with patch.object(deployment, "history", return_value=history):
            self.assertEqual(deployment.guard_database(), history)
        import copy
        for case in ("empty", "failed", "checksum", "old24", "metadata"):
            bad = copy.deepcopy(history)
            if case == "empty":
                bad.clear()
            elif case == "failed":
                bad[0]["success"] = False
            elif case == "checksum":
                bad[0]["checksum"] = "00"
            elif case == "old24":
                bad.extend(dict(history[0], version=str(v)) for v in range(2, 25))
            else:
                bad[0]["execution_time"] = 2
            with self.subTest(case=case), patch.object(deployment, "history", return_value=bad), self.assertRaises(deploy.DeployError):
                deployment.guard_database()

    def test_no_pending_columns_needs_no_append_slot_sql(self):
        deployment = deploy.Deployment(self.args)
        deployment.plan = {"contract": {"deltas": []}}
        with patch.object(deployment, "sql") as sql:
            self.assertEqual(deployment.append_slots(), {})
            sql.assert_not_called()


    def test_compose_image_file_is_explicit_and_env_only_is_single_consumer(self):
        deployment = deploy.Deployment(self.args)
        for image_file, files in ((".env", [str(self.root / ".env")]),
                                  (".images.env", [str(self.root / ".env"), str(self.root / ".images.env")])):
            deployment.plan = {"image_file": image_file}
            command = deployment.compose()
            self.assertEqual([command[i + 1] for i, part in enumerate(command) if part == "--env-file"], files)
            candidate = deployment.compose(self.stage / "images.next")
            actual = [candidate[i + 1] for i, part in enumerate(candidate) if part == "--env-file"]
            self.assertEqual(actual[-1], str(self.stage / "images.next"))
            self.assertEqual(len(actual), len(files))

    def test_client_actions_have_no_reverse_ssh_or_private_signing_key_interface(self):
        base = ["deploy_release_bundle.py", "backup", "--root", str(self.root), "--stage", str(self.stage),
                "--project-name", "test"]
        for option in ("--offhost", "--offhost-file", "--ssh-identity", "--signing-key", "--release"):
            with self.subTest(option=option), patch("sys.argv", [*base, option, "private-untrusted-value"]), \
                    self.assertRaises(deploy.DeployError) as error:
                deploy.parse_args()
            self.assertNotIn("private-untrusted-value", str(error.exception))

    def test_baseline_contract_refuses_unsealed_changes(self):
        contract = json.loads((Path(__file__).resolve().parents[2] /
                               "apps/api/migrations/compose-deployment.json").read_bytes())
        migrations = {"1": "a" * 96}
        deploy.validate_contract(contract, migrations)
        for field, value in (("pending_tail", [2]), ("installed_prefix", 24),
                             ("deltas", [{"version": 2, "additions": [], "replacements": []}]),
                             ("position_pairs", {}),
                             ("schema", "untrusted")):
            with self.subTest(field=field), self.assertRaises(deploy.DeployError):
                deploy.validate_contract(dict(contract, **{field: value}), migrations)
        with self.assertRaises(deploy.DeployError):
            deploy.validate_contract(contract, {str(v): "a" * 96 for v in range(1, 25)})

    def test_baseline_layout_pairs_bind_one_complete_native_layout(self):
        import copy
        from scripts.ci.postgres_migration_contract import canonical_readiness
        contract = {"position_pairs": {"public.t.a": [1, 2], "public.t.b": [2, 3]}}
        fresh = [{"kind": "column", "name": "public.t." + name,
                  "definition": {"position": position, "type": "text"}}
                 for name, position in (("a", 1), ("b", 2))]
        old = copy.deepcopy(fresh)
        for obj in old:
            obj["definition"]["position"] += 1
        self.assertEqual(deploy.baseline_positions(contract, fresh, fresh), {})
        pairs = deploy.baseline_positions(contract, fresh, old)
        canonical_readiness(fresh, old, position_pairs=pairs)
        mixed = copy.deepcopy(old)
        mixed[0]["definition"]["position"] = 1
        with self.assertRaises(deploy.DeployError):
            deploy.baseline_positions(contract, fresh, mixed)
        foreign_type = copy.deepcopy(old)
        foreign_type[0]["definition"]["type"] = "integer"
        with self.assertRaises(ValueError):
            canonical_readiness(fresh, foreign_type,
                                position_pairs=deploy.baseline_positions(contract, fresh, foreign_type))


    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / "stack"
        self.root.mkdir()
        self.backups = self.root / "backups"
        self.backups.mkdir(mode=0o700)
        self.stage = self.backups / "release-20261005T120000Z"
        self.args = argparse.Namespace(root=self.root, stage=self.stage, project_name="isolated-test")

    def journal(self):
        self.stage.mkdir(mode=0o700)
        (self.stage / "journal").mkdir(mode=0o700)
        deployment = deploy.Deployment(self.args)
        deployment.advance("staging")
        return deployment

    def test_only_absolute_existing_non_symlink_root_and_direct_fresh_stage(self):
        for root in (Path("/"), Path("/tmp"), Path("relative"), self.root / "absent"):
            with (
                self.subTest(root=root),
                self.assertRaises((deploy.DeployError, OSError)),
            ):
                deploy.Deployment(argparse.Namespace(root=root, stage=self.stage, project_name="test"))
        linked = Path(self.temporary.name) / "linked"
        linked.symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(deploy.DeployError):
            deploy.Deployment(
                argparse.Namespace(
                    root=linked,
                    stage=linked / "backups" / self.stage.name,
                    project_name="test",
                )
            )
        for stage in (
            self.root / self.stage.name,
            self.backups / "arbitrary",
            self.backups / "nested" / self.stage.name,
        ):
            with self.subTest(stage=stage), self.assertRaises(deploy.DeployError):
                deploy.Deployment(argparse.Namespace(root=self.root, stage=stage, project_name="test"))
        with self.assertRaises(ValueError):
            deploy.Deployment(
                argparse.Namespace(
                    root=self.root,
                    stage=self.backups / "release-20269999T120000Z",
                    project_name="test",
                )
            )

    def test_symlink_ancestor_and_nonregular_leaf_are_refused(self):
        target = self.root / "target"
        target.mkdir()
        (target / "file").write_bytes(b"data")
        linked = self.root / "linked"
        linked.symlink_to(target, target_is_directory=True)
        with self.assertRaises(deploy.DeployError):
            deploy.read(linked / "file")
        with self.assertRaises(deploy.DeployError):
            deploy.read(target)
        fifo = self.root / "fifo"
        os.mkfifo(fifo)
        with self.assertRaises(deploy.DeployError):
            deploy.read(fifo)

    def test_exclusive_private_write_never_overwrites(self):
        target = self.root / "private"
        deploy.write(target, b"first")
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o600)
        with self.assertRaises(deploy.DeployError):
            deploy.write(target, b"second")
        self.assertEqual(target.read_bytes(), b"first")

    def test_root_lock_serializes_distinct_stages_and_refuses_symlinks(self):
        first = deploy.Deployment(self.args)
        second = deploy.Deployment(
            argparse.Namespace(
                root=self.root,
                stage=self.backups / "release-20261005T120001Z",
                project_name="isolated-test",
            )
        )
        with first.lock(), self.assertRaises(deploy.ConcurrentOperation), second.lock():
            self.fail("concurrent root deployment entered its critical section")
        with second.lock():
            pass
        lock = self.root / ".deploy-release.lock"
        lock.unlink()
        lock.symlink_to(self.root / "other")
        with self.assertRaises(OSError), first.lock():
            self.fail("symlinked deployment lock opened")

    def test_root_lock_rechecks_inode_and_metadata_after_flock(self):
        lock = self.root / ".deploy-release.lock"
        retained = self.root / "retained-lock"
        native_flock = deploy.fcntl.flock
        for change in ("hardlink", "replacement", "symlink", "permissions"):
            with self.subTest(change=change):
                lock.unlink(missing_ok=True)
                retained.unlink(missing_ok=True)

                def interleaved_flock(descriptor, operation):
                    if change == "hardlink":
                        os.link(lock, retained)
                    elif change == "permissions":
                        lock.chmod(0o644)
                    else:
                        lock.rename(retained)
                        if change == "symlink":
                            lock.symlink_to(retained)
                        else:
                            deploy.write(lock, b"")
                    return native_flock(descriptor, operation)

                with patch.object(deploy.fcntl, "flock", side_effect=interleaved_flock):
                    with self.assertRaises(deploy.DeployError), deploy.Deployment(self.args).lock():
                        self.fail("a changed deployment lock admitted a protected operation")

    def test_journal_interruption_cannot_skip_or_replay_phase(self):
        deployment = self.journal()
        self.assertEqual(deployment.phase(), "staging")
        with self.assertRaises(deploy.DeployError):
            deployment.advance("backup-starting")
        deployment.advance("staged")
        with self.assertRaises(deploy.DeployError):
            deployment.advance("staged")
        deployment.advance("backup-starting")
        with self.assertRaises(deploy.DeployError):
            deployment.load("staged")
        self.assertEqual(deployment.phase(), "backup-starting")

    def test_seal_detects_tampered_candidate_plan_and_symlinks(self):
        deployment = self.journal()
        for name in ("plan.json", ".images.env.next"):
            target = self.stage / name
            deploy.write(target, b"original")
            seal = "seal-" + name.replace(".", "_") + ".json"
            deployment.seal(seal, [name])
            deployment.verify_seal(seal)
            target.write_bytes(b"changed")
            with self.assertRaises(deploy.DeployError):
                deployment.verify_seal(seal)
            target.unlink()
            target.symlink_to(self.root / "other")
            with self.assertRaises((deploy.DeployError, OSError)):
                deployment.verify_seal(seal)

    def test_image_file_preserves_nonimage_bytes_and_comments(self):
        refs = {role: f"registry.example/product/{role}@sha256:{'a' * 64}" for role in deploy.ROLES}
        source = (
            b"# operational comment\r\nOTHER_KEY=unchanged\r\n"
            b"  PIPING_BACKEND_IMAGE=registry.example/backend:old # pinned\r\n"
            b"export PIPING_FRONTEND_IMAGE=registry.example/frontend:old\n"
            b"PIPING_ANSIBLE_RUNNER_IMAGE=registry.example/runner:old"
        )
        rendered = deploy.render_images(source, refs)
        self.assertTrue(rendered.startswith(b"# operational comment\r\nOTHER_KEY=unchanged\r\n"))
        self.assertIn(b" # pinned\r\n", rendered)
        self.assertIn(b"export PIPING_FRONTEND_IMAGE=", rendered)
        self.assertTrue(rendered.endswith(refs["ansible-runner"].encode()))
        with self.assertRaises(deploy.DeployError):
            deploy.render_images(source + b"\nPIPING_BACKEND_IMAGE=duplicate", refs)
        with self.assertRaises(deploy.DeployError):
            deploy.render_images(b"OTHER_KEY=unchanged\n", refs)
        with self.assertRaises(deploy.DeployError):
            deploy.render_images(b"OTHER_KEY=first\nOTHER_KEY=second\n" + source, refs)

    def test_host_interpolation_overrides_are_not_inherited(self):
        with patch.dict(
            os.environ,
            {
                "PIPING_BACKEND_IMAGE": "wrong",
                "COMPOSE_FILE": "wrong",
                "POSTGRES_USER": "wrong",
                "HOME": str(self.root),
            },
        ):
            sanitized = deploy.environment()
        self.assertNotIn("PIPING_BACKEND_IMAGE", sanitized)
        self.assertNotIn("COMPOSE_FILE", sanitized)
        self.assertNotIn("POSTGRES_USER", sanitized)
        self.assertEqual(sanitized["HOME"], str(self.root))

    def test_packaged_migration_readme_does_not_change_sql_checksums(self):
        readme = tarfile.TarInfo("migrations/README.md")
        readme.size = 32
        self.assertEqual(
            deploy.migration_checksums(migration_archive(extra=readme)),
            deploy.migration_checksums(migration_archive()),
        )
        marker = tarfile.TarInfo("migrations/.keep")
        self.assertEqual(
            deploy.migration_checksums(migration_archive(extra=marker)),
            deploy.migration_checksums(migration_archive()),
        )
        marker.size = 1
        with self.assertRaises(deploy.DeployError):
            deploy.migration_checksums(migration_archive(extra=marker))

    def test_packaged_operator_metadata_cannot_change_runtime_migration_authority(self):
        metadata = tarfile.TarInfo("migrations/maintenance-transition.json")
        metadata.size = 64  # Untrusted non-JSON bytes are never a deployment input.
        observed = deploy.migration_artifact(migration_archive(count=1, extra=metadata))
        expected = deploy.migration_artifact(migration_archive(count=1))
        self.assertEqual(observed, expected)
        self.assertEqual(set(observed[0]), {"1"})
        self.assertIsNone(observed[1])

    def test_sqlx_checksums_are_sha384_of_published_bytes(self):
        checksums = deploy.migration_checksums(migration_archive())
        self.assertEqual(set(checksums), {str(version) for version in range(1, 20)})
        self.assertEqual(checksums["1"], hashlib.sha384(b"-- migration 1\nSELECT 1;\n").hexdigest())
        with self.assertRaises(deploy.DeployError):
            deploy.migration_checksums(migration_archive(omit=18))

    def test_new_forward_migration_comes_from_published_image(self):
        checksums = deploy.migration_checksums(migration_archive(count=20))
        self.assertEqual(len(checksums), 20)
        self.assertEqual(checksums["20"], hashlib.sha384(b"-- migration 20\nSELECT 20;\n").hexdigest())

    def test_migration_archive_requires_nonempty_contiguous_history(self):
        for archive in (migration_archive(count=0), migration_archive(omit=1), migration_archive(omit=10)):
            with self.subTest(size=len(archive)), self.assertRaises(deploy.DeployError):
                deploy.migration_checksums(archive)

    def test_migration_archive_rejects_links_paths_duplicates_and_foreign_files(self):
        extras = []
        for name in (
            "/migrations/0020_test.sql",
            "migrations/../0020_test.sql",
            "migrations/nested/0020_test.sql",
            "migrations/readme.txt",
            "migrations/0001_test.sql",
            "migrations/01_duplicate.sql",
            "migrations/0021_extra.sql",
        ):
            item = tarfile.TarInfo(name)
            item.size = 1
            extras.append(item)
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE):
            item = tarfile.TarInfo("migrations/link")
            item.type, item.linkname = kind, "/outside"
            extras.append(item)
        for item in extras:
            with (
                self.subTest(name=item.name, kind=item.type),
                self.assertRaises(deploy.DeployError),
            ):
                deploy.migration_checksums(migration_archive(item))

    def test_native_command_failure_does_not_disclose_private_output(self):
        with self.assertRaises(deploy.DeployError) as error:
            deploy.run(["sh", "-c", "printf sensitive-output >&2; exit 7"])
        self.assertNotIn("sensitive-output", str(error.exception))

    def test_argument_error_does_not_echo_untrusted_values(self):
        parser = deploy.BoundaryParser()
        parser.add_argument("--port", type=int)
        with self.assertRaises(deploy.DeployError) as error:
            parser.parse_args(["--port", "private-argument"])
        self.assertNotIn("private-argument", str(error.exception))



class PreservationAuthorityTests(unittest.TestCase):
    def inventory_manifest(self, package):
        context = {name: "b" * 64 for name in PRESERVATION_CONTEXT}
        context.update(target="synthetic", root="/data/synthetic", project_name="synthetic",
                       application_source=package["commit"], helper_sha256=package["helper_sha256"],
                       release_sha256=package["release_sha256"],
                       postgres_image="sha256:" + "b" * 64, backend_image="sha256:" + "c" * 64)
        now = datetime.now(timezone.utc)
        return {"schema": "piping.compose-preservation/v1", "status": "approved", "context": context,
                "package_binding_sha256": package_binding(package),
                "issued_at": now.isoformat().replace("+00:00", "Z"),
                "expires_at": (now + timedelta(hours=24)).isoformat().replace("+00:00", "Z"),
                "inventory": [], "position_pairs": {}, "check_capture_sha256": None,
                "append_slots": {"public.owned": 2}}

    def test_inventory_only_manifest_retains_binding_freshness_and_rejects_forged_exceptions(self):
        package = {"commit": "a" * 40, "helper_sha256": "b" * 64, "release_sha256": "c" * 64}
        manifest = self.inventory_manifest(package)
        preservation_manifest(manifest, context=manifest["context"], package_sha256=package_binding(package))
        for field in PRESERVATION_CONTEXT:
            with self.subTest(field=field), self.assertRaises(ValueError):
                preservation_manifest(manifest, context=dict(manifest["context"], **{field: "changed"}),
                                      package_sha256=package_binding(package))
        for change in ({"mode": "inventory-only"}, {"position_pairs": {"public.owned.value": [1, 2]}},
                       {"check_capture_sha256": "inventory-only"}, {"expires_at": "2020-01-01T00:00:00Z"},
                       {"package_binding_sha256": "f" * 64}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                preservation_manifest(dict(manifest, **change), context=manifest["context"],
                                      package_sha256=package_binding(package))

    def test_prepare_attachment_without_capture_and_missing_historical_capture(self):
        package = {"commit": "a" * 40, "helper_sha256": "b" * 64, "release_sha256": "c" * 64}
        manifest = self.inventory_manifest(package)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            approval_path = root / "approval.json"
            output = root / "package"
            output.mkdir(mode=0o700)
            deploy.write_json(approval_path, manifest)
            args = argparse.Namespace(package=output, preservation_manifest=approval_path, check_capture=None)
            deploy.attach_preservation(args, package)
            self.assertEqual(package["preservation"]["capture_sha256"], None)
            self.assertFalse((output / "check-capture.json").exists())
            owner = deploy.Deployment.__new__(deploy.Deployment)
            owner.stage = output
            owner.plan = {"preservation_sha256": deploy.file_sha(output / "preservation.json"),
                          "check_capture_sha256": None, "package_binding_sha256": package_binding(package),
                          "installed_append_slots": manifest["append_slots"]}
            with patch.object(owner, "preservation_context", return_value=manifest["context"]):
                self.assertEqual(owner.approval(), manifest)
                changed = copy.deepcopy(manifest)
                changed["inventory"] = [{"kind": "table", "name": "archive.extra", "definition_sha256": "d" * 64}]
                (output / "preservation.json").write_bytes(canonical(changed))
                with self.assertRaisesRegex(deploy.DeployError, "authority bytes changed"):
                    owner.approval()
            manifest["check_capture_sha256"] = "d" * 64
            approval_path.write_bytes(canonical(manifest))
            with self.assertRaisesRegex(deploy.DeployError, "source/capture mismatch"):
                deploy.attach_preservation(args, package)

    def test_baseline_preservation_accepts_exact_empty_append_slots(self):
        package = {"commit": "a" * 40, "helper_sha256": "b" * 64, "release_sha256": "c" * 64}
        owner = deploy.Deployment.__new__(deploy.Deployment)
        owner.plan = {"contract": {"deltas": []}}
        manifest = self.inventory_manifest(package)
        manifest["append_slots"] = owner.append_slots()
        self.assertEqual(manifest["append_slots"], {})
        preservation_manifest(manifest, context=manifest["context"], package_sha256=package_binding(package))
        with tempfile.TemporaryDirectory() as temporary:
            owner.stage = Path(temporary)
            deploy.write_json(owner.stage / "preservation.json", manifest)
            owner.plan.update(preservation_sha256=deploy.file_sha(owner.stage / "preservation.json"),
                              check_capture_sha256=None, package_binding_sha256=package_binding(package),
                              installed_append_slots={})
            with patch.object(owner, "preservation_context", return_value=manifest["context"]):
                self.assertEqual(owner.approval(), manifest)
                owner.plan["installed_append_slots"] = {"public.owned": 2}
                with self.assertRaisesRegex(deploy.DeployError, "layout/capture mismatch"):
                    owner.approval()
        for invalid in (None, [], {"public.owned": 0}, {"public.owned": True}, {"public.owned": 1600}):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                preservation_manifest(dict(manifest, append_slots=invalid), context=manifest["context"],
                                      package_sha256=package_binding(package))

    def test_baseline_declares_no_catalog_delta_and_refuses_old_cohorts(self):
        owner = deploy.Deployment.__new__(deploy.Deployment)
        contract = json.loads((Path(__file__).resolve().parents[2] / "apps/api/migrations/compose-deployment.json").read_bytes())
        owner.plan = {"contract": contract, "installed_history": [{}]}
        self.assertEqual(owner.target_additions({}), [])
        for size in (0, 2, 22, 23, 24):
            owner.plan["installed_history"] = [{}] * size
            with self.subTest(size=size), self.assertRaises(deploy.DeployError):
                owner.target_additions({})


    def test_manifest_binds_exact_context_inventory_and_72hour_freshness(self):
        now = datetime(2026, 10, 15, tzinfo=timezone.utc)
        context = {name: "b" * 64 for name in PRESERVATION_CONTEXT}
        context.update({"target": "synthetic", "root": "/data/synthetic", "project_name": "synthetic",
                        "application_source": "a" * 40, "postgres_image": "sha256:" + "b" * 64,
                        "backend_image": "sha256:" + "c" * 64})
        manifest = {"schema": "piping.compose-preservation/v1", "status": "approved", "context": context,
            "package_binding_sha256": "a" * 64, "issued_at": "2026-10-15T00:00:00Z", "expires_at": "2026-10-17T00:00:00Z",
            "inventory": [{"kind": "column", "name": "legacy.retained.value", "definition_sha256": "b" * 64}],
            "position_pairs": {"public.owned.value": [1, 3]}, "check_capture_sha256": "c" * 64,
            "append_slots": {"public.owned": 7}}
        preservation_manifest(manifest, context=context, package_sha256="a" * 64, now=now)
        for field in PRESERVATION_CONTEXT:
            wrong = dict(context, **{field: "different"})
            with self.subTest(field=field), self.assertRaises(ValueError):
                preservation_manifest(manifest, context=wrong, package_sha256="a" * 64, now=now)
        for field, value in (("status", "draft-review-required"), ("expires_at", "2026-10-14T00:00:00Z"),
                             ("expires_at", "2026-10-19T00:00:00Z"), ("issued_at", "2026-10-16T00:00:00Z"),
                             ("inventory", None), ("inventory", manifest["inventory"] * 2),
                             ("position_pairs", {"public.owned.value": [1, 1]}), ("check_capture_sha256", "bad")):
            with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                preservation_manifest(dict(manifest, **{field: value}), context=context, package_sha256="a" * 64, now=now)

    def test_package_binding_is_identical_across_native_stores_but_not_source_changes(self):
        package = {"commit": "a" * 40, "images": {"backend": {"published_ref": "repo@sha256:abc", "local_id": "store-a"}}}
        other = copy.deepcopy(package)
        other["images"]["backend"]["local_id"] = "store-b"
        self.assertEqual(package_binding(package), package_binding(other))
        other["commit"] = "b" * 40
        self.assertNotEqual(package_binding(package), package_binding(other))

    def test_native_target_guard_detects_hidden_check_binding_and_append_slot_drift(self):
        owner = deploy.Deployment.__new__(deploy.Deployment)
        catalog = [{"kind": "column", "name": "public.owned.value", "definition": {"position": 2}}]
        owner.plan = {"installed_target_catalog": catalog, "installed_append_slots": {"public.owned": 4},
                      "preservation_sha256": "a" * 64, "check_capture_sha256": "b" * 64,
                      "installed_check_bindings": [{"ast": "native-tree"}]}
        with patch.object(owner, "catalog", return_value=catalog), patch.object(owner, "approval"), \
                patch.object(owner, "guard_original_check_identity"), \
                patch.object(owner, "append_slots", return_value={"public.owned": 4}), \
                patch.object(owner, "installed_check_bindings", return_value=[{"ast": "native-tree"}]):
            owner.guard_target_catalog()
        with patch.object(owner, "catalog", return_value=catalog), patch.object(owner, "approval"), \
                patch.object(owner, "append_slots", return_value={"public.owned": 4}), \
                patch.object(owner, "installed_check_bindings", return_value=[{"ast": "different-dependency"}]), \
                self.assertRaises(deploy.DeployError):
            owner.guard_target_catalog()
        with patch.object(owner, "catalog", return_value=catalog), \
                patch.object(owner, "append_slots", return_value={"public.owned": 5}), self.assertRaises(deploy.DeployError):
            owner.guard_target_catalog()

    def test_inventory_only_target_guard_keeps_check_and_layout_drift_guards_without_original_capture(self):
        owner = deploy.Deployment.__new__(deploy.Deployment)
        catalog = [{"kind": "table", "name": "archive.retained", "definition": {"owner": "piping"}}]
        owner.plan = {"installed_target_catalog": catalog, "installed_append_slots": {"public.owned": 2},
                      "preservation_sha256": "a" * 64, "check_capture_sha256": None,
                      "installed_check_bindings": [{"ast": "native-tree"}]}
        with patch.object(owner, "catalog", return_value=catalog), patch.object(owner, "approval"), \
                patch.object(owner, "append_slots", return_value={"public.owned": 2}), \
                patch.object(owner, "guard_original_check_identity", side_effect=AssertionError("no capture")), \
                patch.object(owner, "installed_check_bindings", return_value=[{"ast": "native-tree"}]) as bindings:
            owner.guard_target_catalog()
            bindings.return_value = [{"ast": "changed"}]
            with self.assertRaisesRegex(deploy.DeployError, "CHECK binding changed"):
                owner.guard_target_catalog()


class TargetBindingRegressionTests(unittest.TestCase):
    """Valid signatures do not grant permission to change the approved target."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="target-case-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.root = self.directory / "client"
        self.root.mkdir(mode=0o700)
        (self.root / "backups").mkdir(mode=0o700)
        self.stage = self.root / "backups/release-20261015T120000Z"
        self.stage.mkdir(mode=0o700)
        (self.stage / "journal").mkdir(mode=0o700)
        key = self.directory / "signing.pem"
        deploy.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", str(key)])
        key.chmod(0o600)
        self.key = key
        public = deploy.run(["openssl", "pkey", "-in", str(key), "-pubout"])
        deploy.write(self.stage / "verification-key.pem", public)
        self.package_dir = self.directory / "package"
        self.package_dir.mkdir(mode=0o700)
        self.package = {"commit": "a" * 40, "pipeline_id": "18492"}
        for directory in (self.package_dir, self.stage):
            deploy.write_json(directory / "package.json", self.package)
            deploy.sign(directory / "package.json", key, directory / "package.sig")
        repository = Path(__file__).resolve().parents[2]
        contract = json.loads((repository / "apps/api/migrations/compose-deployment.json").read_bytes())
        migrations = {str(int(p.name.split("_", 1)[0])): hashlib.sha384(p.read_bytes()).hexdigest()
                      for p in (repository / "apps/api/migrations").glob("*.sql")}
        self.plan = {"target": "host-a", "root": str(self.root), "project_name": "test", "image_file": ".env",
            "helper_sha256": deploy.helper_identity(), "package_sha256": deploy.file_sha(self.stage / "package.json"),
            "verification_key_sha256": deploy.file_sha(self.stage / "verification-key.pem"),
            "package_binding_sha256": package_binding(self.package), "preservation_sha256": None,
            "check_capture_sha256": None, "contract": contract, "migrations": migrations,
            "commit": self.package["commit"], "pipeline_id": self.package["pipeline_id"]}
        deploy.write_json(self.stage / "plan.json", self.plan)
        self.owner = deploy.Deployment(argparse.Namespace(root=self.root, stage=self.stage, project_name="test"))
        self.owner.plan = self.plan
        self.owner.advance("staging")
        self.owner.advance("staged")
        self.owner.seal("stage-seal.json", ["plan.json", "package.json", "package.sig", "verification-key.pem"])

    def request(self, target):
        return {"schema": "piping.compose-backup/v1", "target": target, "root": str(self.root),
            "stage": str(self.stage), "project_name": "test", "helper_sha256": deploy.helper_identity(),
            "plan_sha256": deploy.file_sha(self.stage / "plan.json"), "package_sha256": self.plan["package_sha256"],
            "commit": self.plan["commit"], "pipeline_id": self.plan["pipeline_id"], "nonce": "1" * 32,
            "preservation_sha256": None, "files": {}}

    def test_start_and_new_update_refuse_every_unresolved_mutating_phase(self):
        self.owner.guard_recovery()
        for phase in deploy.PHASES[deploy.PHASES.index("backup-starting"):]:
            self.owner.advance(phase)
            unsafe = deploy.PHASES.index(phase) >= deploy.PHASES.index("writers-stopping") and phase != "healthy"
            with self.subTest(phase=phase), patch.object(deploy, "run") as native:
                if unsafe:
                    with self.assertRaises(deploy.RecoveryRequired):
                        self.owner.guard_recovery()
                    for action in (deploy.start_existing, deploy.local_update):
                        args = argparse.Namespace(root=self.root, project_name="test", image=["frontend=fixture:local"], skip_pull=True)
                        with self.assertRaises(deploy.RecoveryRequired):
                            action(args)
                else:
                    self.owner.guard_recovery()
                native.assert_not_called()

    def test_start_selects_only_existing_containers_not_uncreated_optional_profiles(self):
        args = argparse.Namespace(root=self.root, project_name="test")
        services = ["backend", "frontend", "ansible-runner", "nginx", "postgres", "redis"]
        with patch.object(deploy, "run", side_effect=["\n".join(services).encode(), b"", b"ready"]) as native:
            deploy.start_existing(args)
        start = native.call_args_list[1].args[0]
        self.assertEqual(start[start.index("start") + 1:], ["--wait", "--wait-timeout", "180", *services])
        with patch.object(deploy, "run", return_value=b"postgres\nredis") as native:
            with self.assertRaisesRegex(deploy.DeployError, "containers are missing"):
                deploy.start_existing(args)
        self.assertEqual(native.call_count, 1)

    def test_client_host_rename_is_denied_before_backup_or_apply_can_mutate(self):
        original_journal = sorted(p.name for p in (self.stage / "journal").iterdir())
        for action in ("backup", "apply"):
            if action == "apply":
                for phase in deploy.PHASES[2:deploy.PHASES.index("backup-verified") + 1]:
                    self.owner.advance(phase)
            before = sorted(p.name for p in (self.stage / "journal").iterdir())
            with self.subTest(action=action), patch.object(deploy.socket, "gethostname", return_value="host-b"), \
                    patch.object(self.owner, "guard_config", side_effect=AssertionError("wrong host reached mutation preflight")), \
                    patch.object(deploy, "run") as native, self.assertRaisesRegex(deploy.DeployError, "executing host"):
                getattr(self.owner, action)()
            native.assert_not_called()
            self.assertEqual(sorted(p.name for p in (self.stage / "journal").iterdir()), before)
            self.assertFalse((self.stage / "consumed-receipt.json").exists())
        self.assertEqual(original_journal, ["00-staging", "01-staged"])

    def test_leader_rejects_request_target_outside_staged_authority_before_signing(self):
        deploy.write_json(self.stage / "backup-request.json", self.request("host-b"))
        def authenticated_package(_directory, key):
            deploy.authenticate(self.package_dir / "package.json", self.package_dir / "package.sig", key)
            return self.package
        args = argparse.Namespace(dossier=self.stage, package=self.package_dir,
                                  verification_key=self.stage / "verification-key.pem", signing_key=self.key)
        with patch.object(deploy, "verify_package", side_effect=authenticated_package), \
                patch.object(deploy, "private_dossier_directory", return_value=self.stage), \
                patch.object(deploy.socket, "gethostname", return_value="independent-leader"), \
                patch.object(deploy, "sign") as signer, \
                self.assertRaisesRegex(deploy.DeployError, "backup request identity mismatch"):
            deploy.verify_backup(args)
        signer.assert_not_called()
        self.assertFalse((self.stage / "restore-receipt.sig").exists())

    def test_same_client_target_loads_real_signed_stage(self):
        with patch.object(deploy.socket, "gethostname", return_value="host-a"):
            self.owner.load("staged")
        self.assertEqual(self.owner.plan["target"], "host-a")
        self.assertEqual(self.owner.phase(), "staged")

    def test_leader_hostname_is_not_used_as_the_client_target(self):
        deploy.write_json(self.stage / "backup-request.json", self.request("host-a"))
        def authenticated_package(_directory, key):
            deploy.authenticate(self.package_dir / "package.json", self.package_dir / "package.sig", key)
            return self.package
        args = argparse.Namespace(dossier=self.stage, package=self.package_dir,
                                  verification_key=self.stage / "verification-key.pem", signing_key=self.key)
        # This deliberately incomplete dossier must pass the target boundary on
        # an independent leader, then fail at the next real byte-inventory guard.
        with patch.object(deploy, "verify_package", side_effect=authenticated_package), \
                patch.object(deploy, "private_dossier_directory", return_value=self.stage), \
                patch.object(deploy.socket, "gethostname", return_value="independent-leader"), \
                self.assertRaisesRegex(deploy.DeployError, "incomplete backup dossier"):
            deploy.verify_backup(args)

    def test_local_dossier_accepts_private_disk_and_rejects_public_storage(self):
        with tempfile.TemporaryDirectory(prefix="piping-dossier-", dir=Path(__file__).resolve().parents[2]) as parent:
            # Fixture privacy must not depend on flags inherited from a shared checkout.
            Path(parent).chmod(0o2700)
            with tempfile.TemporaryDirectory(dir=parent) as temporary:
                directory = Path(temporary)
                directory.chmod(0o700)
                self.assertEqual(deploy.private_dossier_directory(directory), directory)
                directory.chmod(0o755)
                with self.assertRaisesRegex(deploy.DeployError, "private"):
                    deploy.private_dossier_directory(directory)

    def test_valid_leader_signature_for_host_b_cannot_consume_host_a_stage(self):
        request = self.request("host-b")
        deploy.write_json(self.stage / "backup-request.json", request)
        receipt = {"schema": "piping.compose-restore-receipt/v1", "result": "native-restore-migration-conservation-passed",
                   "request": request, "request_sha256": deploy.file_sha(self.stage / "backup-request.json")}
        path, signature = self.directory / "receipt.json", self.directory / "receipt.sig"
        deploy.write_json(path, receipt)
        deploy.sign(path, self.key, signature)
        self.owner.args.receipt, self.owner.args.receipt_signature = path, signature
        deploy.authenticate(path, signature, self.stage / "verification-key.pem")
        with patch.object(deploy.socket, "gethostname", return_value="host-b"), \
                self.assertRaisesRegex(deploy.DeployError, "restore receipt binding mismatch"):
            self.owner.consume_receipt()
        self.assertFalse((self.stage / "consumed-receipt.json").exists())


class CheckSourceWiringTests(unittest.TestCase):
    def owner(self):
        owner = deploy.Deployment.__new__(deploy.Deployment)
        owner.stage = Path("/unused")
        owner.plan = {"commit": "96c9833e6a4cfdee0eff9c206fcd2cbee9e8dd53",
                      "prefix_manifest_sha384": {"actual-image-path.sql": "a" * 96},
                      "target_postgres_image": "sha256:" + "b" * 64,
                      "installed_check_bindings": [], "installed_target_catalog": []}
        return owner

    def test_inventory_only_source_accepts_exact_canonical_plus_extras_without_historical_proof(self):
        owner = self.owner()
        canonical = [{"kind": "column", "name": "public.owned.value",
                      "definition": {"position": 1, "type": "text"}},
                     {"kind": "constraint", "name": "public.owned.check", "definition": "CHECK (true)"}]
        extra = {"kind": "table", "name": "archive.retained", "definition": {"owner": "piping"}}
        installed = copy.deepcopy(canonical) + [extra]
        inventory = [{"kind": extra["kind"], "name": extra["name"],
                      "definition_sha256": deploy.definition_hash(extra["definition"])}]
        approval = {"check_capture_sha256": None, "position_pairs": {}, "inventory": inventory}
        owner.plan.update(installed_target_catalog=installed, installed_append_slots={"public.owned": 1})
        with patch.object(owner, "approval", return_value=approval), \
                patch.object(owner, "database_target", return_value=nullcontext()), \
                patch.object(owner, "append_slots", return_value={"public.owned": 1}), \
                patch.object(owner, "native_sql_session", side_effect=AssertionError("historical proof forbidden")), \
                patch.object(deploy, "write_json"):
            owner.source_readiness(canonical, {})
            for failure in ("omitted", "changed", "unlisted", "position", "check", "missing"):
                with self.subTest(failure=failure):
                    approval["inventory"] = inventory if failure != "omitted" else []
                    owner.plan["installed_target_catalog"] = copy.deepcopy(installed)
                    if failure == "changed":
                        owner.plan["installed_target_catalog"][-1]["definition"]["owner"] = "other"
                    elif failure == "unlisted":
                        owner.plan["installed_target_catalog"].append(dict(extra, name="archive.unlisted"))
                    elif failure == "position":
                        owner.plan["installed_target_catalog"][0]["definition"]["position"] = 2
                    elif failure == "check":
                        owner.plan["installed_target_catalog"][1]["definition"] = "CHECK (false)"
                    elif failure == "missing":
                        owner.plan["installed_target_catalog"].pop(0)
                    with self.assertRaises(ValueError):
                        owner.source_readiness(canonical, {})

    def test_inventory_only_cannot_authorize_historical_check_roundtrip(self):
        owner = self.owner()
        form = next(iter(deploy.historical_check_forms().values()))
        before = [{"kind": "constraint", "name": form.name, "definition": form.historical}]
        after = [{"kind": "constraint", "name": form.name, "definition": form.canonical}]
        owner.plan.update(installed_target_catalog=before, source_roundtrip_checks={})
        with patch.object(owner, "catalog", return_value=after), \
                patch.object(owner, "approval", return_value={"check_capture_sha256": None}), \
                patch.object(owner, "native_sql_session", return_value=nullcontext(lambda q: "")), \
                self.assertRaisesRegex(deploy.DeployError, "unapproved native CHECK roundtrip"):
            owner.roundtrip_target({})

    def test_source_readiness_supplies_same_plan_binding_to_native_and_catalog_guards(self):
        owner = self.owner()
        capture = b'{"bindings":[]}'
        with patch.object(owner, "approval", return_value={"check_capture_sha256": "c" * 64,
                "inventory": [], "position_pairs": {}}), \
                patch.object(deploy, "read", return_value=capture), \
                patch.object(owner, "native_sql_session", return_value=nullcontext(lambda q: "")), \
                patch.object(deploy, "prove_original_native_checks", return_value=()) as native, \
                patch.object(deploy, "canonical_readiness") as readiness:
            owner.source_readiness([], {})
        expected = dict(source_sha=owner.plan["commit"], prefix_sha384=owner.plan["prefix_manifest_sha384"],
                        postgres_image=owner.plan["target_postgres_image"])
        for key, value in expected.items():
            self.assertEqual(native.call_args.kwargs[key], value)
            self.assertEqual(readiness.call_args.kwargs[key], value)

    def test_target_roundtrip_keeps_actual_candidate_binding(self):
        owner = self.owner()
        form = next(iter(deploy.historical_check_forms().values()))
        before = [{"kind": "constraint", "name": form.name, "definition": form.historical}]
        after = [{"kind": "constraint", "name": form.name, "definition": form.canonical}]
        owner.plan.update(installed_target_catalog=before, source_roundtrip_checks={})
        with patch.object(owner, "catalog", return_value=after), \
                 patch.object(owner, "approval", return_value={"check_capture_sha256": "c" * 64}), \
                patch.object(owner, "native_sql_session", return_value=nullcontext(
                    lambda q: json.dumps([{"name": form.name, "definition": form.canonical}]))), \
                patch.object(deploy, "prove_native_check") as native:
            self.assertEqual(owner.roundtrip_target({}), after)
        self.assertEqual(native.call_args.kwargs, dict(source_sha=owner.plan["commit"],
            postgres_image=owner.plan["target_postgres_image"], prefix_sha384=owner.plan["prefix_manifest_sha384"]))


class ArchiveDeliveryContractTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.archive = self.root / "image.tar"
        self.layer = b"native layer bytes"
        self.config = {"os": "linux", "architecture": "amd64",
                       "config": {"Labels": {"org.opencontainers.image.revision": "a" * 40}},
                       "rootfs": {"type": "layers", "diff_ids": ["sha256:" + deploy.sha(self.layer)]}}

    def image_archive(self, *, config=None, layer=None, extra=None):
        content = canonical(config or self.config)
        digest = "sha256:" + deploy.sha(content)
        with tarfile.open(self.archive, "w") as archive:
            for name, value in (("manifest.json", canonical([{"Config": "config.json", "Layers": ["layer.tar"]}])),
                                ("config.json", content), ("layer.tar", self.layer if layer is None else layer)):
                item = tarfile.TarInfo(name)
                item.size = len(value)
                archive.addfile(item, io.BytesIO(value))
            if extra:
                archive.addfile(extra, io.BytesIO(b"x" * extra.size) if extra.isfile() else None)
        return digest

    def test_archive_verifies_bytes_platform_revision_and_ordered_diffids(self):
        config = self.image_archive()
        self.assertEqual(save_identity(self.archive, config, "linux/amd64", "a" * 40), self.config)
        for platform, revision in (("linux/arm64", "a" * 40), ("linux/amd64", "b" * 40)):
            with self.subTest(platform=platform, revision=revision), self.assertRaises(ValueError):
                save_identity(self.archive, config, platform, revision)
        with self.assertRaises(ValueError):
            save_identity(self.archive, "sha256:" + "0" * 64, "linux/amd64")
        self.image_archive(layer=b"changed layer")
        with self.assertRaises(ValueError):
            save_identity(self.archive, config, "linux/amd64")

    def test_archive_rejects_native_tar_path_links_and_duplicate_metadata(self):
        for name, kind in (("../escape", tarfile.REGTYPE), ("/escape", tarfile.REGTYPE),
                           ("config.json", tarfile.REGTYPE), ("link", tarfile.SYMTYPE),
                           ("hardlink", tarfile.LNKTYPE), ("device", tarfile.CHRTYPE)):
            member = tarfile.TarInfo(name)
            member.type = kind
            member.linkname = "config.json"
            with self.subTest(name=name, kind=kind):
                config = self.image_archive(extra=member)
                with self.assertRaises(ValueError):
                    save_identity(self.archive, config, "linux/amd64")

    def test_two_layer_order_is_checked_against_config_diff_ids(self):
        first, second = b"layer one", b"layer two"
        config = copy.deepcopy(self.config)
        config["rootfs"]["diff_ids"] = ["sha256:" + deploy.sha(first), "sha256:" + deploy.sha(second)]
        config_bytes = canonical(config)
        for layers in (("one.tar", "two.tar"), ("two.tar", "one.tar")):
            with tarfile.open(self.archive, "w") as archive:
                for name, value in (("manifest.json", canonical([{"Config": "config.json", "Layers": layers}])),
                                    ("config.json", config_bytes), ("one.tar", first), ("two.tar", second)):
                    item = tarfile.TarInfo(name)
                    item.size = len(value)
                    archive.addfile(item, io.BytesIO(value))
            with self.subTest(layers=layers):
                if layers[0] == "one.tar":
                    save_identity(self.archive, "sha256:" + deploy.sha(config_bytes), "linux/amd64")
                else:
                    with self.assertRaises(ValueError):
                        save_identity(self.archive, "sha256:" + deploy.sha(config_bytes), "linux/amd64")

    def test_registry_manifest_is_separate_authority_from_local_config(self):
        config = self.image_archive()
        raw = canonical({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
                         "config": {"digest": config}})
        ref = "registry.example/backend@sha256:" + deploy.sha(raw)
        verify_manifest(raw, ref, config)
        with self.assertRaises(ValueError):
            verify_manifest(raw + b" ", ref, config)
        with self.assertRaises(ValueError):
            verify_manifest(raw, ref, "sha256:" + "f" * 64)

    def test_offline_native_root_must_reach_actual_config_even_for_an_index(self):
        config = self.image_archive()
        native = canonical({"schemaVersion": 2, "config": {"digest": config}})
        native_id = "sha256:" + deploy.sha(native)
        index = canonical({"schemaVersion": 2, "manifests": [{"digest": native_id}]})
        index_id = "sha256:" + deploy.sha(index)
        with tarfile.open(self.archive, "a") as archive:
            for identity, value in ((native_id, native), (index_id, index)):
                item = tarfile.TarInfo("blobs/sha256/" + identity.removeprefix("sha256:"))
                item.size = len(value)
                archive.addfile(item, io.BytesIO(value))
        save_authority(self.archive, index_id, config)
        save_authority(self.archive, native_id, config)
        with self.assertRaises(ValueError):
            save_authority(self.archive, "sha256:" + "0" * 64, config)
        with self.assertRaises(ValueError):
            save_authority(self.archive, index_id, "sha256:" + "f" * 64)

    def test_native_openssl_signature_and_immutable_receipt_consumption(self):
        key, public = self.root / "signing.pem", self.root / "verification.pem"
        deploy.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", str(key)])
        key.chmod(0o600)
        deploy.write(public, deploy.run(["openssl", "pkey", "-in", str(key), "-pubout"]))
        stack = self.root / "stack"
        stack.mkdir()
        (stack / "backups").mkdir(mode=0o700)
        stage = stack / "backups/release-20261014T120000Z"
        stage.mkdir(mode=0o700)
        deployment = deploy.Deployment(argparse.Namespace(root=stack, stage=stage, project_name="test"))
        deploy.write(stage / "verification-key.pem", public.read_bytes())
        deploy.write_json(stage / "plan.json", {"test": "plan"})
        deploy.write(stage / "offline-pre-upgrade.dump", b"actual dump bytes")
        request = {"helper_sha256": deploy.helper_identity(), "package_sha256": "p" * 64, "preservation_sha256": None,
                   "plan_sha256": deploy.file_sha(stage / "plan.json"), "target": socket.gethostname(),
                   "root": str(stack), "stage": str(stage), "project_name": "test", "commit": "a" * 40,
                   "pipeline_id": "18492", "nonce": "1" * 32,
                   "files": {"offline-pre-upgrade.dump": {"sha256": deploy.file_sha(stage / "offline-pre-upgrade.dump"),
                                                          "size": 17}}}
        deployment.plan = {"target": request["target"], "package_sha256": request["package_sha256"], "preservation_sha256": None, "commit": request["commit"],
                           "pipeline_id": request["pipeline_id"]}
        deploy.write_json(stage / "backup-request.json", request)
        receipt = {"schema": "piping.compose-restore-receipt/v1", "result": "native-restore-migration-conservation-passed",
                   "request": request, "request_sha256": deploy.file_sha(stage / "backup-request.json")}
        path, signature = self.root / "receipt.json", self.root / "receipt.sig"
        deployment.args.receipt, deployment.args.receipt_signature = path, signature
        deploy.write_json(path, receipt)
        deploy.sign(path, key, signature)
        original = path.read_bytes()
        path.write_bytes(original + b" ")
        with self.assertRaises(deploy.DeployError):
            deployment.consume_receipt()
        path.write_bytes(original)
        for field in ("nonce", "target", "root", "stage", "project_name", "helper_sha256", "commit", "pipeline_id", "preservation_sha256",
                      "package_sha256", "plan_sha256"):
            bad = copy.deepcopy(receipt)
            bad["request"][field] = "different"
            bad_path, bad_signature = self.root / (field + ".json"), self.root / (field + ".sig")
            deploy.write_json(bad_path, bad)
            deploy.sign(bad_path, key, bad_signature)
            deployment.args.receipt, deployment.args.receipt_signature = bad_path, bad_signature
            with self.subTest(field=field), self.assertRaises(deploy.DeployError):
                deployment.consume_receipt()
            self.assertFalse((stage / "consumed-receipt.json").exists())
        deployment.args.receipt, deployment.args.receipt_signature = path, signature
        dump = stage / "offline-pre-upgrade.dump"
        dump.write_bytes(b"changed dump bytes")
        with self.assertRaises(deploy.DeployError):
            deployment.consume_receipt()
        dump.write_bytes(b"actual dump bytes")
        deployment.consume_receipt()
        self.assertTrue((stage / "consumed-receipt.json").is_file())
        with self.assertRaises(deploy.DeployError):
            deployment.consume_receipt()


if __name__ == "__main__":
    unittest.main()
