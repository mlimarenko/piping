"""Behaviour of the release-bundle deploy adapter on Piping.App's three images.

The adapter is the only place the validated release contract is turned into the
Kustomize image block of the Argo CD manifests. A wrong digest, a stale commit
or a partial image set must leave the manifests untouched: the deploy job would
otherwise push a release that does not match the artifacts its gates passed.

Every case below writes its fixtures into a temporary directory and runs the
adapter with the same argument shape the CI deploy lane uses, so the manifests
under test are disposable copies, never the tracked worktree.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
ADAPTER = REPOSITORY_ROOT / "scripts" / "ci" / "apply_release_bundle.py"
COMMIT = "c" * 40
PIPELINE_ID = "123"
REGISTRY_PATH = "gitlab.piping.space:4567/piping-space/products/piping/piping.app"
BACKEND = f"{REGISTRY_PATH}/release/backend"
FRONTEND = f"{REGISTRY_PATH}/release/frontend"
ANSIBLE_RUNNER = f"{REGISTRY_PATH}/release/ansible-runner"
MUTABLE_TAG = "d" * 40


def image(artifact_key: str, repository: str, digest_nibble: str) -> dict:
    digest = f"sha256:{digest_nibble * 64}"
    return {
        "artifact_key": artifact_key,
        "repository": repository,
        "digest": digest,
        "digest_ref": f"{repository}@{digest}",
        "platforms": "linux/amd64",
        "build_job": f"image:{artifact_key}:build",
    }


def bundle() -> dict:
    return {
        "schema": "ci-components.release/v1",
        "release_id": "release-123-cccccccccccc",
        "version": "1.2.3",
        "commit": COMMIT,
        "pipeline_id": PIPELINE_ID,
        "files": [],
        "images": [
            image("backend", BACKEND, "a"),
            image("frontend", FRONTEND, "b"),
            image("ansible-runner", ANSIBLE_RUNNER, "e"),
        ],
    }


KUSTOMIZATION = f"""apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - deployment.yaml
images:
  - name: piping-frontend
    newName: {REGISTRY_PATH}/frontend
    newTag: "{MUTABLE_TAG}"
  # Preserve this operational comment.
  - name: piping-backend
    newName: {REGISTRY_PATH}/backend
    newTag: "{MUTABLE_TAG}"
  - name: piping-ansible-runner
    newName: {REGISTRY_PATH}/ansible-runner
    newTag: "{MUTABLE_TAG}"
"""


class DeployReleaseBundleTests(unittest.TestCase):
    def run_adapter(
        self,
        release: dict,
        kustomization: str = KUSTOMIZATION,
        *,
        commit: str = COMMIT,
        pipeline_id: str = PIPELINE_ID,
    ) -> tuple[subprocess.CompletedProcess[str], str]:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            release_path = root / "release.json"
            kustomization_path = root / "kustomization.yaml"
            release_path.write_text(json.dumps(release), encoding="utf-8")
            kustomization_path.write_text(kustomization, encoding="utf-8")
            completed = subprocess.run(
                [
                    sys.executable,
                    str(ADAPTER),
                    "--release",
                    str(release_path),
                    "--kustomization",
                    str(kustomization_path),
                    "--commit",
                    commit,
                    "--pipeline-id",
                    pipeline_id,
                    "--platform",
                    "linux/amd64",
                    "--image",
                    f"backend={BACKEND}",
                    "--image",
                    f"frontend={FRONTEND}",
                    "--image",
                    f"ansible-runner={ANSIBLE_RUNNER}",
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            return completed, kustomization_path.read_text(encoding="utf-8")

    def test_replaces_all_three_mutable_tags_with_release_digests(self) -> None:
        completed, rendered = self.run_adapter(bundle())
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertNotIn("newTag:", rendered)
        self.assertEqual(rendered.count('digest: "sha256:'), 3)
        for nibble in ("a", "b", "e"):
            self.assertIn(f'digest: "sha256:{nibble * 64}"', rendered)
        self.assertIn("# Preserve this operational comment.", rendered)

    def test_is_idempotent_for_an_already_digest_pinned_kustomization(self) -> None:
        first, rendered = self.run_adapter(bundle())
        self.assertEqual(first.returncode, 0, first.stderr)
        second, rerendered = self.run_adapter(bundle(), rendered)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(rerendered, rendered)

    def test_moves_repositories_and_digests_together_without_changing_selectors(self) -> None:
        completed, rendered = self.run_adapter(bundle())
        self.assertEqual(completed.returncode, 0, completed.stderr)
        for role, repository in (
            ("backend", BACKEND), ("frontend", FRONTEND), ("ansible-runner", ANSIBLE_RUNNER)
        ):
            self.assertIn(f"  - name: piping-{role}\n", rendered)
            self.assertIn(f'    newName: "{repository}"\n', rendered)
        self.assertNotIn(f"newName: {REGISTRY_PATH}/backend", rendered)

    def test_rejects_ambiguous_or_missing_repository_targets_without_editing(self) -> None:
        name = f"    newName: {REGISTRY_PATH}/backend\n"
        for target in ("", name + name, "    newName: https://unexpected.invalid/backend\n"):
            with self.subTest(target=target):
                source = KUSTOMIZATION.replace(name, target)
                completed, rendered = self.run_adapter(bundle(), source)
                self.assertEqual(completed.returncode, 2, completed.stderr)
                self.assertEqual(rendered, source)

    def test_file_artifacts_are_rejected_without_changing_manifests(self) -> None:
        for files in (None, {}, [{"artifact_key": "package"}]):
            with self.subTest(files=files):
                release = bundle()
                release["files"] = files
                completed, rendered = self.run_adapter(release)
                self.assertEqual(completed.returncode, 2, completed.stderr)
                self.assertEqual(rendered, KUSTOMIZATION)

    def test_rejects_a_bundle_from_another_commit_without_editing(self) -> None:
        release = bundle()
        release["commit"] = "e" * 40
        completed, rendered = self.run_adapter(release)
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertIn("another commit", completed.stderr)
        self.assertEqual(rendered, KUSTOMIZATION)

    def test_rejects_a_bundle_from_another_pipeline_without_editing(self) -> None:
        release = bundle()
        release["pipeline_id"] = "124"
        completed, rendered = self.run_adapter(release)
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertIn("another pipeline", completed.stderr)
        self.assertEqual(rendered, KUSTOMIZATION)

    def test_rejects_a_bundle_from_another_repository_without_editing(self) -> None:
        release = bundle()
        release["images"][0]["repository"] = f"{REGISTRY_PATH}/backend-mirror"
        completed, rendered = self.run_adapter(release)
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertIn("repository mismatch", completed.stderr)
        self.assertEqual(rendered, KUSTOMIZATION)

    def test_rejects_an_incomplete_image_set_without_editing(self) -> None:
        release = bundle()
        release["images"] = release["images"][:2]
        completed, rendered = self.run_adapter(release)
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertIn("incomplete", completed.stderr)
        self.assertEqual(rendered, KUSTOMIZATION)

    def test_requires_the_real_image_producer_without_a_gate_alias(self) -> None:
        for producer in ("image:frontend:build", "image:backend:gate", ""):
            with self.subTest(producer=producer):
                release = bundle()
                release["images"][0]["build_job"] = producer
                completed, rendered = self.run_adapter(release)
                self.assertEqual(completed.returncode, 2, completed.stderr)
                self.assertIn("build identity mismatch", completed.stderr)
                self.assertEqual(rendered, KUSTOMIZATION)
        release = bundle()
        release["images"][0]["gate_job"] = release["images"][0].pop("build_job")
        completed, rendered = self.run_adapter(release)
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertIn("unsupported schema", completed.stderr)
        self.assertEqual(rendered, KUSTOMIZATION)

    def test_rejects_an_unexpected_image_without_editing(self) -> None:
        release = bundle()
        release["images"][0]["artifact_key"] = "cache"
        completed, rendered = self.run_adapter(release)
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertIn("unexpected image", completed.stderr)
        self.assertEqual(rendered, KUSTOMIZATION)

    def test_rejects_malformed_image_entries_without_editing(self) -> None:
        for broken in ([["not", "a", "mapping"]], "not-a-list"):
            with self.subTest(broken=broken):
                release = bundle()
                release["images"] = broken
                completed, rendered = self.run_adapter(release)
                self.assertEqual(completed.returncode, 2, completed.stderr)
                self.assertEqual(rendered, KUSTOMIZATION)

        release = bundle()
        release["images"][0]["extra"] = True
        completed, rendered = self.run_adapter(release)
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertIn("unsupported schema", completed.stderr)
        self.assertEqual(rendered, KUSTOMIZATION)

    def test_rejects_duplicate_or_ambiguous_kustomize_entries(self) -> None:
        duplicate = KUSTOMIZATION + "  - name: piping-backend\n    newTag: duplicate\n"
        completed, rendered = self.run_adapter(bundle(), duplicate)
        self.assertEqual(completed.returncode, 2, completed.stderr)
        self.assertEqual(rendered, duplicate)


if __name__ == "__main__":
    unittest.main()
