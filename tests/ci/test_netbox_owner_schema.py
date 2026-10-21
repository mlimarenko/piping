"""SQL slice invariants that do not require Docker or a Rust build."""

import hashlib
import json
import pathlib
import tempfile
import unittest
from unittest.mock import patch

from scripts import check_netbox_owner_schema as schema
from scripts.check_netbox_owner_schema import (
    FIXTURES, ROOT, Proof,
    identifier, manifest, proof_output_directory, source_provenance,
)


class OwnerSchemaContract(unittest.TestCase):
    def test_identifier_boundary(self):
        self.assertEqual(identifier("owner_upgrade"), '"owner_upgrade"')
        self.assertEqual(identifier("_abs_weight"), '"_abs_weight"')
        for unsafe in ("", "a;DROP TABLE x", "x.y", "x'", "../x", "a-b", "A"):
            with self.subTest(unsafe=unsafe), self.assertRaises(ValueError):
                identifier(unsafe)

    def test_runtime_is_single_baseline(self):
        schema.validate_baseline()
        self.assertEqual(list(manifest()), [1])


    def test_manifest_rejects_empty_gapped_and_duplicate_history(self):
        for names in ((), ("0001_first.sql", "0003_gap.sql"),
                      ("0001_first.sql", "0001_duplicate.sql")):
            with self.subTest(names=names), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                for name in names:
                    (root / name).touch()
                with patch.object(schema, "MIGRATIONS", root), self.assertRaises(ValueError):
                    manifest()


    def test_foreign_checksum_is_rejected_before_forward_ddl(self):
        proof = Proof.__new__(Proof)
        with patch.object(proof, "history", side_effect=AssertionError("history disagrees")), \
                patch.object(proof, "sql") as sql:
            with self.assertRaisesRegex(AssertionError, "history disagrees"):
                proof.apply("test", 1)
            sql.assert_not_called()

    def test_postgres_service_is_explicit_and_cannot_target_customer(self):
        for host in ("10.51.7.115", "localhost", "postgres;DROP", ""):
            with self.subTest(host=host), self.assertRaises(ValueError):
                Proof(None, ROOT / "unused-output", postgres_host=host)
        with self.assertRaises(ValueError):
            Proof("proof", ROOT / "unused-output", postgres_host="postgres")

    def test_schema_catalog_rejects_lossy_object_identities(self):
        proof = Proof.__new__(Proof)
        objects = [{"kind": "column", "name": "truncated", "definition": "one"},
                   {"kind": "column", "name": "truncated", "definition": "two"}]
        with patch.object(proof, "scalar", return_value=json.dumps(objects)):
            with self.assertRaisesRegex(AssertionError, "lossless and unique"):
                proof.schema("test")


class ProofOutputBoundary(unittest.TestCase):
    def test_native_service_uses_only_job_scoped_coverage_scratch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory) / "checkout"
            coverage = root / "apps/api/coverage"
            coverage.mkdir(parents=True)
            with patch("scripts.check_netbox_owner_schema.ROOT", root):
                output = coverage / ".netbox-schema-184516"
                self.assertEqual(proof_output_directory(output, "postgres"), output)
                for rejected in (
                    coverage / "netbox-schema-proof", coverage / ".netbox-schema-0",
                    coverage / ".netbox-schema-x", coverage / "nested/.netbox-schema-184516",
                    root / "apps/api/migrations/.netbox-schema-184516",
                    pathlib.Path(directory) / ".netbox-schema-184516",
                    pathlib.Path("/data/artifacts/.netbox-schema-184516"),
                ):
                    with self.subTest(path=rejected), self.assertRaises(ValueError):
                        proof_output_directory(rejected, "postgres")

    def test_native_scratch_cannot_follow_a_parent_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory) / "checkout"
            (root / "apps/api").mkdir(parents=True)
            outside = pathlib.Path(directory) / "outside"
            outside.mkdir()
            (root / "apps/api/coverage").symlink_to(outside, target_is_directory=True)
            with patch("scripts.check_netbox_owner_schema.ROOT", root), self.assertRaises(ValueError):
                proof_output_directory(root / "apps/api/coverage/.netbox-schema-184516", "postgres")

    def test_local_container_keeps_out_of_tree_data_boundary(self):
        self.assertEqual(proof_output_directory(pathlib.Path("/data/artifacts/owner-schema-proof"), None),
                         pathlib.Path("/data/artifacts/owner-schema-proof"))
        for rejected in (ROOT / "apps/api/coverage/.netbox-schema-184516", pathlib.Path("/tmp/proof")):
            with self.subTest(path=rejected), self.assertRaises(ValueError):
                proof_output_directory(rejected, None)

    def test_native_scratch_cannot_follow_a_leaf_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory) / "checkout"
            coverage = root / "apps/api/coverage"
            coverage.mkdir(parents=True)
            output = coverage / ".netbox-schema-184516"
            output.symlink_to(coverage / ".netbox-schema-184517", target_is_directory=True)
            with patch("scripts.check_netbox_owner_schema.ROOT", root), self.assertRaises(ValueError):
                proof_output_directory(output, "postgres")


class SourceProvenanceContract(unittest.TestCase):
    head = "a" * 40

    def observe(self, *, dirty=False, mismatch=False, require_clean=False):
        def git(argv, **kwargs):
            if argv[-2:] == ["rev-parse", "HEAD"]:
                return (self.head + "\n").encode()
            if "status" in argv:
                return b" M scripts/check_netbox_owner_schema.py\n" if dirty else b""
            if "show" in argv:
                path = argv[-1].split(":", 1)[1]
                data = (ROOT / path).read_bytes()
                return data + b"changed" if mismatch else data
            raise AssertionError("Unexpected Git observation")

        with patch("scripts.check_netbox_owner_schema.subprocess.check_output", side_effect=git):
            return source_provenance(require_clean)

    def test_clean_source_validates_head_and_all_input_hashes(self):
        source = self.observe(require_clean=True)
        self.assertEqual(source["validated_source_sha"], self.head)
        self.assertFalse(source["worktree_dirty"])
        self.assertTrue(source["files_match_head"])
        catalog_path = "scripts/ci/postgres_migration_contract.py"
        self.assertEqual(source["file_sha256"][catalog_path],
                         hashlib.sha256((ROOT / catalog_path).read_bytes()).hexdigest())
        for fixture in (schema.MIGRATIONS / "maintenance-transition.json", FIXTURES / "netbox_forward_schema.sql"):
            self.assertIn(fixture.relative_to(ROOT).as_posix(), source["file_sha256"])
        self.assertEqual(source["file_sha256"], source["head_file_sha256"])

    def test_dirty_source_does_not_validate_head(self):
        source = self.observe(dirty=True)
        self.assertEqual(source["observed_head"], self.head)
        self.assertTrue(source["worktree_dirty"])
        self.assertIsNone(source["validated_source_sha"])

    def test_clean_status_with_mismatched_bytes_does_not_validate_head(self):
        source = self.observe(mismatch=True)
        self.assertFalse(source["worktree_dirty"])
        self.assertFalse(source["files_match_head"])
        self.assertIsNone(source["validated_source_sha"])

    def test_final_mode_rejects_dirty_or_mismatched_source(self):
        for args in ({"dirty": True}, {"mismatch": True}):
            with self.subTest(args=args), self.assertRaisesRegex(ValueError, "Final proof requires"):
                self.observe(require_clean=True, **args)


if __name__ == "__main__":
    unittest.main()
