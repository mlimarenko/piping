"""Scope decisions against disposable real Git histories, not YAML strings."""

import hashlib
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "helper_scope", Path(__file__).resolve().parents[2] / "scripts/ci/verify_helper_source_scope.py",
)
SCOPE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCOPE)


class HelperScopeTests(unittest.TestCase):
    def setUp(self):
        # Tiny disposable histories honor the runtime's tempfile directory;
        # CI must not depend on a workstation-only /data mount.
        self.temp = tempfile.TemporaryDirectory(prefix="helper-scope-")
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.git("init", "-b", "develop")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "user.name", "Scope Fixture")
        self.write("scripts/deploy_release_bundle.py", "helper\n")
        self.write("apps/api/product.rs", "product\n")
        self.base = self.save()
        self.git("update-ref", "refs/remotes/origin/develop", self.base)
        self.git("checkout", "-b", "helper")
        self.env = {
            "CI_PIPELINE_SOURCE": "push", "CI_COMMIT_BRANCH": "helper",
            "CI_DEFAULT_BRANCH": "develop",
        }

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.repo), *args], check=True, capture_output=True).stdout.decode().strip()

    def write(self, path, text):
        file = self.repo / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text)

    def save(self):
        self.git("add", "--all")
        self.git("commit", "-m", "fixture")
        return self.git("rev-parse", "HEAD")

    def check(self):
        self.env["CI_COMMIT_SHA"] = self.git("rev-parse", "HEAD")
        return SCOPE.verify(self.repo, self.env)

    def test_helper_edit_uses_default_merge_base(self):
        self.write("scripts/deploy_release_bundle.py", "changed\n")
        head = self.save()
        self.assertEqual(self.check(), (self.base, head, 1))

    def test_add_and_delete_regular_owned_files(self):
        (self.repo / "scripts/deploy_release_bundle.py").unlink()
        self.write("scripts/ci/compose_delivery_contract.py", "contract\n")
        self.save()
        self.assertEqual(self.check()[2], 2)

    def test_shared_preservation_module_and_focused_test_are_owned(self):
        self.write("scripts/ci/postgres_migration_contract.py", "shared contract\n")
        self.write("tests/ci/test_postgres_migration_contract.py", "focused contract proof\n")
        head = self.save()
        self.assertEqual(self.check(), (self.base, head, 2))
        self.write("scripts/ci/unreviewed_schema_contract.py", "unknown sibling\n")
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_shared_module_changes_do_not_authorize_product_sql(self):
        self.write("scripts/ci/postgres_migration_contract.py", "shared contract\n")
        self.write("apps/api/migrations/0023_secret_field_lifecycle_management.sql", "product SQL\n")
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_public_source_publication_diff_is_owned_on_mr_and_default_push(self):
        workflows = (".github/workflows/release-docker-images.yml",)
        for path in workflows:
            self.write(path, "former GitHub artifact publication\n")
        self.base = self.save()
        self.git("update-ref", "refs/remotes/origin/develop", self.base)
        self.write("scripts/ci/publish_public_release.py", "source publisher\n")
        self.write("tests/ci/test_public_release.py", "real Git publisher proof\n")
        self.write("apps/desktop/README.md", "private GitLab artifacts\n")
        self.write("apps/desktop/SIGNING.md", "operator signing setup\n")
        for path in workflows:
            (self.repo / path).unlink()
        head = self.save()
        self.env.update(CI_PIPELINE_SOURCE="merge_request_event",
                        CI_MERGE_REQUEST_TARGET_BRANCH_NAME="develop",
                        CI_MERGE_REQUEST_TARGET_BRANCH_PROTECTED="true")
        self.assertEqual(self.check(), (self.base, head, 5))
        self.env.update(CI_PIPELINE_SOURCE="push", CI_COMMIT_BRANCH="develop",
                        CI_COMMIT_REF_PROTECTED="true", CI_COMMIT_BEFORE_SHA=self.base)
        self.assertEqual(self.check(), (self.base, head, 5))

    def test_publisher_changes_do_not_authorize_product_code_or_unlisted_siblings(self):
        for path in ("apps/api/product.rs", "apps/desktop/package.json",
                     "apps/desktop/unreviewed.md", ".github/workflows/unreviewed.yml",
                     "scripts/ci/product-release.yml"):
            with self.subTest(path=path):
                self.git("checkout", "--detach", self.base)
                self.write("scripts/ci/publish_public_release.py", "source publisher\n")
                self.write(path, "unreviewed product or policy change\n")
                self.save()
                with self.assertRaises(SCOPE.ScopeError):
                    self.check()
                self.env.update(CI_PIPELINE_SOURCE="push", CI_COMMIT_BRANCH="develop",
                                CI_COMMIT_REF_PROTECTED="true", CI_COMMIT_BEFORE_SHA=self.base)
                with self.assertRaises(SCOPE.ScopeError):
                    self.check()
                self.env.update(CI_PIPELINE_SOURCE="push", CI_COMMIT_BRANCH="helper")

    def test_public_distribution_and_licensing_fixture_have_bounded_ownership(self):
        paths = (
            "scripts/ci/public-release.yml", "scripts/ci/select_public_release.py",
            "scripts/ci/github_public_release.py", "tests/ci/test_github_public_release.py",
            "scripts/public_release.py",
            "tests/ci/test_public_artifacts.py", "scripts/license-e2e-fixture.sh",
            "tests/compose/docker-compose.licensing.yml", "tests/ci/test_license_fixture.py",
        )
        for path in paths:
            self.write(path, "reviewed distribution and fixture change\n")
        head = self.save()
        self.assertEqual(self.check(), (self.base, head, len(paths)))
        self.env.update(CI_PIPELINE_SOURCE="push", CI_COMMIT_BRANCH="develop",
                        CI_COMMIT_REF_PROTECTED="true", CI_COMMIT_BEFORE_SHA=self.base)
        self.assertEqual(self.check(), (self.base, head, len(paths)))
        self.write("apps/api/migrations/0023_unreviewed.sql", "product schema change\n")
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_installer_and_compose_changes_are_owned_without_authorizing_product_code(self):
        self.write("install.sh", "#!/bin/sh\nexit 0\n")
        (self.repo / "install.sh").chmod(0o755)
        self.write("docker-compose.yml", "services: {}\n")
        self.write("env/images.env", "PIPING_BACKEND_IMAGE=fixture:reviewed\n")
        head = self.save()
        self.assertEqual(self.check(), (self.base, head, 3))
        self.write("apps/api/product.rs", "unreviewed product change\n")
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_installer_ci_policy_does_not_authorize_release_policy_changes(self):
        self.write("scripts/ci/install-source.yml", "include: []\n")
        head = self.save()
        self.assertEqual(self.check(), (self.base, head, 1))
        self.write("scripts/ci/product-release.yml", "include: []\n")
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_installed_nginx_configuration_does_not_authorize_backend_image_producer(self):
        self.write("docker/nginx/nginx.conf", "events {}\nhttp {}\n")
        self.write("docker/nginx/locations/backend.conf", "location /api/ { proxy_pass http://backend:7000; }\n")
        head = self.save()
        self.assertEqual(self.check(), (self.base, head, 2))
        self.write("apps/api/Dockerfile", "FROM fixture:product\n")
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_mixed_product_and_unknown_paths_rejected(self):
        for path in ("apps/api/product.rs", "unreviewed.py", "Cargo.lock", "scripts/ci/product-release.yml"):
            with self.subTest(path=path):
                self.write("scripts/deploy_release_bundle.py", path)
                self.write(path, "unreviewed\n")
                self.save()
                with self.assertRaises(SCOPE.ScopeError):
                    self.check()

    def test_product_rename_into_owned_path_rejected(self):
        (self.repo / "scripts/ci").mkdir()
        self.git("mv", "apps/api/product.rs", "scripts/ci/compose_delivery_contract.py")
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_reviewed_product_route_relocation_only(self):
        # Historical authorization must not depend on today's product policy.
        reviewed = "include: []\n# reviewed historical route fixture\n"
        digest = hashlib.sha256(reviewed.encode()).hexdigest()
        with patch.object(SCOPE, "PRODUCT_ROUTE_SHA256", digest):
            self.write("scripts/ci/product-release.yml", reviewed)
            self.save()
            self.assertEqual(self.check()[2], 1)
            self.write("scripts/ci/product-release.yml", reviewed + "# changed\n")
            self.save()
            with self.assertRaises(SCOPE.ScopeError):
                self.check()

    def test_nonancestor_before_rejected(self):
        self.write("scripts/deploy_release_bundle.py", "changed\n")
        head = self.save()
        self.git("checkout", "--orphan", "unrelated")
        other = self.save()
        self.git("checkout", "helper")
        self.env.update(CI_COMMIT_BRANCH="develop", CI_COMMIT_REF_PROTECTED="true", CI_COMMIT_BEFORE_SHA=other)
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_ambiguous_merge_base_rejected(self):
        self.write("scripts/deploy_release_bundle.py", "left\n")
        left = self.save()
        self.git("checkout", "-b", "right", self.base)
        self.write("scripts/deploy_release_bundle.py", "right\n")
        right = self.save()
        tree = self.git("rev-parse", "HEAD^{tree}")
        merge_left = self.git("commit-tree", tree, "-p", left, "-p", right, "-m", "merge left")
        merge_right = self.git("commit-tree", tree, "-p", right, "-p", left, "-m", "merge right")
        self.git("update-ref", "refs/remotes/origin/develop", merge_left)
        self.git("checkout", "--detach", merge_right)
        with self.assertRaisesRegex(SCOPE.ScopeError, "unique merge base"):
            self.check()

    def test_cli_default_push_accepts_and_rejects_exact_checkout(self):
        self.write("scripts/deploy_release_bundle.py", "changed\n")
        head = self.save()
        env = dict(os.environ, **self.env)
        env.update(CI_COMMIT_BRANCH="develop", CI_COMMIT_REF_PROTECTED="true", CI_COMMIT_BEFORE_SHA=self.base, CI_COMMIT_SHA=head)
        script = Path(SCOPE.__file__)
        result = subprocess.run([str(script)], cwd=self.repo, env=env, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(head.encode(), result.stdout)
        env["CI_COMMIT_SHA"] = self.base
        result = subprocess.run([str(script)], cwd=self.repo, env=env, capture_output=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn(b"checkout does not match", result.stderr)

    def test_owned_rename_to_unknown_rejected(self):
        self.git("mv", "scripts/deploy_release_bundle.py", "unlisted.py")
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_mode_change_and_symlink_rejected(self):
        (self.repo / "scripts/deploy_release_bundle.py").chmod(0o755)
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()
        (self.repo / "scripts/deploy_release_bundle.py").unlink()
        (self.repo / "scripts/deploy_release_bundle.py").symlink_to("../apps/api/product.rs")
        self.save()
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_default_push_only_checks_before_to_after(self):
        self.git("checkout", "develop")
        self.write("apps/api/product.rs", "already released\n")
        released = self.save()
        self.write("scripts/deploy_release_bundle.py", "helper change\n")
        head = self.save()
        self.env.update(CI_COMMIT_BRANCH="develop", CI_COMMIT_REF_PROTECTED="true", CI_COMMIT_BEFORE_SHA=released)
        self.assertEqual(self.check(), (released, head, 1))
        self.env["CI_COMMIT_BEFORE_SHA"] = self.base
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_missing_zero_or_unrelated_before_rejected(self):
        self.write("scripts/deploy_release_bundle.py", "changed\n")
        self.save()
        self.env.update(CI_COMMIT_BRANCH="develop", CI_COMMIT_REF_PROTECTED="true")
        for value in ("", "0" * 40, "1" * 40):
            with self.subTest(value=value):
                self.env["CI_COMMIT_BEFORE_SHA"] = value
                with self.assertRaises(SCOPE.ScopeError):
                    self.check()

    def test_missing_default_reference_rejected(self):
        self.write("scripts/deploy_release_bundle.py", "changed\n")
        self.save()
        self.git("update-ref", "-d", "refs/remotes/origin/develop")
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_shallow_and_checkout_mismatch_rejected(self):
        self.write("scripts/deploy_release_bundle.py", "changed\n")
        head = self.save()
        self.env["CI_COMMIT_SHA"] = self.base
        with self.assertRaises(SCOPE.ScopeError):
            SCOPE.verify(self.repo, self.env)
        (self.repo / ".git/shallow").write_text(head + "\n")
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_mr_requires_protected_default_target(self):
        self.write("scripts/deploy_release_bundle.py", "changed\n")
        self.save()
        self.env.update(CI_PIPELINE_SOURCE="merge_request_event", CI_MERGE_REQUEST_TARGET_BRANCH_NAME="develop", CI_MERGE_REQUEST_TARGET_BRANCH_PROTECTED="true")
        self.assertEqual(self.check()[0], self.base)
        self.env["CI_MERGE_REQUEST_TARGET_BRANCH_PROTECTED"] = "false"
        with self.assertRaises(SCOPE.ScopeError):
            self.check()

    def test_empty_and_web_pipeline_rejected(self):
        with self.assertRaises(SCOPE.ScopeError):
            self.check()
        self.write("scripts/deploy_release_bundle.py", "changed\n")
        self.save()
        self.env["CI_PIPELINE_SOURCE"] = "web"
        with self.assertRaises(SCOPE.ScopeError):
            self.check()


if __name__ == "__main__":
    unittest.main()
