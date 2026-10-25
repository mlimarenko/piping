"""Maintenance command boundaries; native transactions are exercised by schema proof."""

import contextlib
import copy
import io
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

from scripts import rebaseline_sqlx as maintenance


class MaintenanceBoundaryTests(unittest.TestCase):
    def test_installed_layout_rejects_duplicate_object_identity(self):
        contract = json.loads(maintenance.CONTRACT.read_text())
        contract["installed_layout"]["columns"].append(
            copy.deepcopy(contract["installed_layout"]["columns"][0]))
        with self.assertRaisesRegex(ValueError, "Duplicate installed-layout"):
            maintenance.installed_catalog_digest_sql(contract)

    def test_installed_layout_rejects_invalid_physical_position(self):
        for value in (0, -1, True, "1"):
            contract = json.loads(maintenance.CONTRACT.read_text())
            contract["installed_layout"]["columns"][0]["from"] = value
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "Invalid installed-layout"):
                maintenance.installed_catalog_digest_sql(contract)

    def test_installed_layout_must_bind_the_existing_historical_catalog(self):
        contract = json.loads(maintenance.CONTRACT.read_text())
        contract["installed_layout"]["provenance"]["reference_catalog_sha256"] = "00" * 32
        with self.assertRaisesRegex(ValueError, "bind the historical source"):
            maintenance.installed_catalog_digest_sql(contract)

    def test_mutation_requires_both_operator_acknowledgements_before_contact(self):
        for flags in ([], ["--backup-confirmed"], ["--maintenance-confirmed"]):
            with self.subTest(flags=flags), patch.object(maintenance.subprocess, "run") as run:
                with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                    maintenance.main([*flags, "apply-env"])
                self.assertEqual(error.exception.code, 2)
                run.assert_not_called()

    def test_libpq_credentials_never_enter_arguments_or_echoed_diagnostics(self):
        result = subprocess.CompletedProcess([], 3, b"ROLLBACK", b"private diagnostic")
        output = io.StringIO()
        with patch.object(maintenance.subprocess, "run", return_value=result) as run, \
                contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            code = maintenance.main(["--backup-confirmed", "--maintenance-confirmed", "apply-env"])
        self.assertEqual(code, 3)
        self.assertEqual(run.call_args.args[0], ["psql", "-X", "-q", "-v", "ON_ERROR_STOP=1"])
        self.assertNotIn("private diagnostic", output.getvalue())
        self.assertNotIn("Maintenance committed", output.getvalue())

    def test_connection_url_is_not_an_accepted_argument(self):
        with patch.object(maintenance.subprocess, "run") as run:
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                maintenance.main(["--backup-confirmed", "--maintenance-confirmed", "apply-env", "untrusted-url"])
            run.assert_not_called()

    def test_complete_history_preserves_the_original_frozen_oracle(self):
        contract = json.loads(maintenance.CONTRACT.read_text())
        frozen = json.loads((maintenance.ROOT / "apps/api/tests/fixtures/netbox/frozen_migration_prefix.json").read_text())
        historical = {row["version"]: row for row in contract["history"]}
        for filename, checksum in frozen["sha384"].items():
            row = historical[int(Path(filename).name[:4])]
            self.assertEqual(row["checksum"], checksum)
            self.assertEqual(row["description"], Path(filename).stem[5:].replace("_", " "))


if __name__ == "__main__":
    unittest.main()
