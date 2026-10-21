from __future__ import annotations

import base64
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
RELEASE_WRAPPER_PATH = REPOSITORY_ROOT / "scripts" / "ci-rust-release.sh"
COVERAGE_RUNNER_PATH = REPOSITORY_ROOT / "scripts" / "ci-rust-coverage.sh"
BUILD_SCRIPT_PATH = REPOSITORY_ROOT / "apps" / "api" / "build.rs"


def generate_key_pair(algorithm: str) -> tuple[bytes, bytes]:
    with tempfile.TemporaryDirectory() as temporary_directory:
        private_key = Path(temporary_directory) / "private.pem"
        public_key = Path(temporary_directory) / "public.pem"
        command = ["openssl", "genpkey", "-algorithm", algorithm]
        if algorithm == "RSA":
            command.extend(["-pkeyopt", "rsa_keygen_bits:2048"])
        subprocess.run(
            [*command, "-out", str(private_key)],
            check=True,
            capture_output=True,
            timeout=30,
        )
        subprocess.run(
            ["openssl", "pkey", "-in", str(private_key), "-pubout", "-out", str(public_key)],
            check=True,
            capture_output=True,
            timeout=30,
        )
        return public_key.read_bytes(), private_key.read_bytes()


class CiSecurityContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.valid_ed25519_key, cls.valid_ed25519_private_key = generate_key_pair(
            "Ed25519"
        )
        cls.rsa_key, _ = generate_key_pair("RSA")
        cls.build_script_directory = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.build_script_directory.cleanup)
        cls.build_script_binary = (
            Path(cls.build_script_directory.name) / "piping-api-build-script"
        )
        subprocess.run(
            [
                "rustc",
                "--edition=2024",
                str(BUILD_SCRIPT_PATH),
                "-o",
                str(cls.build_script_binary),
            ],
            cwd=REPOSITORY_ROOT,
            check=True,
            capture_output=True,
            timeout=60,
        )

    def test_rust_test_readiness_is_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            scripts_directory = root / "scripts"
            scripts_directory.mkdir()
            runner = scripts_directory / COVERAGE_RUNNER_PATH.name
            runner.write_bytes(COVERAGE_RUNNER_PATH.read_bytes())
            binary_directory = root / "bin"
            cargo_marker = root / "cargo-called"
            binary_directory.mkdir()
            commands = {
                "seq": "printf '1\\n'",
                "pg_isready": "exit 1",
                "sleep": "exit 0",
                "cargo": f'touch "{cargo_marker}"',
            }
            for name, body in commands.items():
                executable = binary_directory / name
                executable.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
                executable.chmod(0o755)

            completed = subprocess.run(
                ["bash", str(runner), "test"],
                cwd=root,
                env={
                    **os.environ,
                    "CI_PROJECT_DIR": str(root),
                    "PATH": f"{binary_directory}:{os.environ['PATH']}",
                },
                check=False,
                capture_output=True,
                text=True,
                timeout=10,
            )
            cargo_was_called = cargo_marker.exists()

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("test dependencies did not become ready", completed.stderr)
        self.assertFalse(cargo_was_called, "tests ran without their services")

    def test_wrapper_accepts_real_ed25519_and_drops_encoded_input(self) -> None:
        encoded = base64.b64encode(self.valid_ed25519_key).decode("ascii")
        completed, captured = self.run_release_wrapper(encoded=encoded)

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertNotIn(encoded, completed.stdout + completed.stderr)
        self.assertIs(captured["called"], True)
        self.assertEqual(captured["pem"], self.valid_ed25519_key.rstrip(b"\n"))
        self.assertEqual(captured["base64"], b"")
        self.assertEqual(
            captured["args"].decode("utf-8").splitlines(),
            ["build", "--locked", "--workspace", "--release"],
        )

    def test_wrapper_rejects_bad_base64_before_cargo(self) -> None:
        completed, captured = self.run_release_wrapper(encoded="%%%not-base64%%%")

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("base64", completed.stderr)
        self.assertIs(captured["called"], False)

    def test_wrapper_checks_release_targets_without_optimizing_a_discarded_binary(self) -> None:
        completed, captured = self.run_release_wrapper(
            raw=self.valid_ed25519_key,
            operation="check",
            arguments=("--workspace", "--release", "--all-targets"),
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(
            captured["args"].decode("utf-8").splitlines(),
            ["check", "--locked", "--workspace", "--release", "--all-targets"],
        )
        self.assertEqual(captured["pem"], self.valid_ed25519_key)
        self.assertEqual(captured["base64"], b"")

    def test_wrapper_rejects_an_unknown_cargo_operation(self) -> None:
        completed, captured = self.run_release_wrapper(
            raw=self.valid_ed25519_key, operation="publish"
        )
        self.assertEqual(completed.returncode, 2)
        self.assertFalse(captured["called"])

    def test_wrapper_rejects_rsa_before_cargo(self) -> None:
        encoded = base64.b64encode(self.rsa_key).decode("ascii")
        completed, captured = self.run_release_wrapper(encoded=encoded)

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("Ed25519", completed.stderr)
        self.assertIs(captured["called"], False)

    def test_wrapper_rejects_malformed_pem_before_cargo(self) -> None:
        malformed = (
            b"-----BEGIN PUBLIC KEY-----\n"
            b"not-a-valid-subject-public-key-info\n"
            b"-----END PUBLIC KEY-----\n"
        )
        encoded = base64.b64encode(malformed).decode("ascii")
        completed, captured = self.run_release_wrapper(encoded=encoded)

        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("Ed25519", completed.stderr)
        self.assertIs(captured["called"], False)

    def test_release_key_rejects_trailing_material_before_cargo(self) -> None:
        contaminated_keys = {
            "arbitrary text": self.valid_ed25519_key + b"unexpected trailing text",
            "private key": self.valid_ed25519_key + self.valid_ed25519_private_key,
        }

        for label, contaminated_key in contaminated_keys.items():
            wrapper, captured = self.run_release_wrapper(raw=contaminated_key)
            build_script = self.run_build_script(public_key=contaminated_key)
            with self.subTest(label=label, entrypoint="wrapper"):
                self.assertNotEqual(wrapper.returncode, 0)
                self.assertIs(captured["called"], False)
            with self.subTest(label=label, entrypoint="build script"):
                self.assertNotEqual(build_script.returncode, 0)

    def test_direct_release_build_script_rejects_missing_and_rsa_keys(self) -> None:
        missing = self.run_build_script()
        rsa = self.run_build_script(public_key=self.rsa_key)

        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("required", missing.stderr)
        self.assertNotEqual(rsa.returncode, 0)
        self.assertIn("Ed25519", rsa.stderr)

    def test_direct_release_build_script_accepts_ed25519(self) -> None:
        completed = self.run_build_script(public_key=self.valid_ed25519_key)

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertNotIn(
            self.valid_ed25519_key.decode("ascii"),
            completed.stdout + completed.stderr,
        )

    def run_release_wrapper(
        self, *, encoded: str | None = None, raw: bytes | None = None,
        operation: str = "build", arguments: tuple[str, ...] = (),
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, bytes | bool]]:
        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary_path = Path(temporary_directory)
            fake_bin = temporary_path / "bin"
            capture_dir = temporary_path / "capture"
            fake_bin.mkdir()
            capture_dir.mkdir()
            fake_cargo = fake_bin / "cargo"
            fake_cargo.write_text(
                "#!/bin/sh\n"
                'touch "$CAPTURE_DIR/called"\n'
                'printf \'%s\\n\' "$@" > "$CAPTURE_DIR/args"\n'
                'printf \'%s\' "$PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM" > "$CAPTURE_DIR/pem"\n'
                'printenv PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM_B64 > "$CAPTURE_DIR/base64" 2>/dev/null || :\n',
                encoding="utf-8",
            )
            fake_cargo.chmod(0o755)
            environment = {**os.environ, "CAPTURE_DIR": str(capture_dir)}
            environment.pop("PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM", None)
            environment.pop("PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM_B64", None)
            environment["PATH"] = f"{fake_bin}:{environment['PATH']}"
            if encoded is not None:
                environment["PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM_B64"] = encoded
            if raw is not None:
                environment["PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM"] = raw.decode("ascii")

            completed = subprocess.run(
                ["bash", str(RELEASE_WRAPPER_PATH), operation, *arguments],
                cwd=REPOSITORY_ROOT,
                env=environment,
                check=False,
                capture_output=True,
                text=True,
                timeout=30,
            )
            captured: dict[str, bytes | bool] = {
                "called": (capture_dir / "called").exists(),
                "args": (capture_dir / "args").read_bytes()
                if (capture_dir / "args").exists()
                else b"",
                "pem": (capture_dir / "pem").read_bytes()
                if (capture_dir / "pem").exists()
                else b"",
                "base64": (capture_dir / "base64").read_bytes()
                if (capture_dir / "base64").exists()
                else b"",
            }
            return completed, captured

    def run_build_script(
        self, *, public_key: bytes | None = None
    ) -> subprocess.CompletedProcess[str]:
        environment = {**os.environ}
        environment.pop("PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM", None)
        environment["CARGO_MANIFEST_DIR"] = str(REPOSITORY_ROOT / "apps" / "api")
        environment["PROFILE"] = "release"
        if public_key is not None:
            environment["PIPING_LICENSE_BAKED_PUBLIC_KEY_PEM"] = public_key.decode(
                "ascii"
            )
        return subprocess.run(
            [str(self.build_script_binary)],
            cwd=REPOSITORY_ROOT,
            env=environment,
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )


if __name__ == "__main__":
    unittest.main()
