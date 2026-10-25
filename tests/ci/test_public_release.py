"""Real Git publication tests; no network or private application objects leave fixtures."""

from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from scripts.ci import publish_public_release as publisher
from scripts.ci import select_public_release as selector
from scripts.ci.apply_release_bundle import AdapterError

WORKFLOW = (
    "name: Publish public images\n"
    "on:\n"
    "  release:\n"
    "    types: [published]\n"
    "jobs:\n"
    "  publish:\n"
    "    runs-on: ubuntu-latest\n"
    "    steps:\n"
    "      - run: echo publish\n"
)
INSTALL_PUBLIC = '#!/bin/sh\nPUBLIC_VERSION="__PIPING_PUBLIC_VERSION__"\n'
README = (
    "# Piping\n\n"
    "curl -fsSL https://github.com/mlimarenko/piping/raw/__PIPING_PUBLIC_VERSION__/"
    "install.sh | sh\n\n"
    "Documentation stays as committed.\n"
)


class PublicReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="pip136-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "private"
        self.source.mkdir()
        self.remote = self.root / "public.git"
        self.remote.mkdir()
        self.scratch = self.root / "scratch"
        self.scratch.mkdir()
        self.git(self.source, "init", "--quiet", "--initial-branch=develop")
        self.git(self.source, "config", "user.name", "Fixture")
        self.git(self.source, "config", "user.email", "fixture@example.invalid")
        self.git(self.remote, "init", "--quiet", "--bare", "--initial-branch=main")
        (self.source / "secret-history").write_text("deleted private history\n")
        self.commit("Internal ancestor")
        self.ancestor = self.git(self.source, "rev-parse", "HEAD").strip()
        (self.source / "secret-history").unlink()
        (self.source / "env").mkdir()
        (self.source / "env/images.env").write_text("STALE_COHORT=true\n")
        (self.source / ".github/workflows").mkdir(parents=True)
        (self.source / ".github/workflows/release-docker-images.yml").write_text(WORKFLOW)
        (self.source / ".github/workflows/build.yml").write_text("must not be exported\n")
        (self.source / ".github/workflows/desktop.yml").write_text("must not be exported\n")
        (self.source / ".github/CONTRIBUTING.md").write_text("source contribution instructions\n")
        (self.source / "scripts").mkdir()
        (self.source / "install.sh").write_text(INSTALL_PUBLIC)
        (self.source / "install.sh").chmod(0o755)
        (self.source / "README.md").write_text(README)
        (self.source / "run.sh").write_text("#!/bin/sh\necho committed\n")
        (self.source / "run.sh").chmod(0o755)
        (self.source / "current").symlink_to("run.sh")
        self.commit("Private selected release")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.release = self.root / "selection.json"
        self.save_selection()
        self.initial_refs = self.refs()

    def test_empty_remote_creates_only_parentless_bare_100_root(self):
        self.assertEqual(self.refs(), "")
        result = self.publish()
        self.assertIsNone(result["parent"])
        self.assertEqual(self.git(self.remote, "rev-list", "--count", "main").strip(), "1")
        self.assertEqual(self.git(self.remote, "show", "-s", "--format=%P", "main").strip(), "")
        self.assertEqual(self.git(self.remote, "rev-parse", "1.0.0").strip(), result["public_commit"])
        self.assertEqual(len(self.refs().splitlines()), 2)
        self.assertEqual(self.publish()["status"], "unchanged")

    def git(self, directory, *arguments, data=None):
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
                   GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
        result = subprocess.run(["git", "-C", str(directory), *arguments], input=data,
                                text=True, capture_output=True, env=env, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def git_status(self, directory, *arguments):
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull)
        return subprocess.run(["git", "-C", str(directory), *arguments],
                              capture_output=True, env=env, timeout=20).returncode

    def commit(self, message):
        self.git(self.source, "add", "--all")
        self.git(self.source, "commit", "--quiet", "-m", message)

    def reservation(self, version, sha, pipeline_iid="123"):
        major, minor, patch = version.split(".")
        return {
            "version": version, "core": version,
            "major": major, "minor": minor, "patch": patch,
            "prerelease": "", "build_metadata": "", "channel": "stable",
            "source": "release", "release_line": f"{major}.{minor}",
            "release_bump": "patch", "git_tag": version, "image_tag": version,
            "is_release": False, "is_release_candidate": True, "is_prerelease": False,
            "build_id": pipeline_iid, "git_sha": sha, "tag_reserved": True,
        }

    def selection(self, version, sha, pipeline_id="123", pipeline_iid="123"):
        return {
            "schema": "piping.public-selection/v1", "commit": sha,
            "pipeline_id": pipeline_id, "pipeline_iid": pipeline_iid,
            "reservation": self.reservation(version, sha, pipeline_iid),
        }

    def save_selection(self):
        self.release.write_text(json.dumps(self.selection_document))

    def publish(self, **overrides):
        args = dict(source=self.source, release=self.release, expected_commit=self.sha,
                    expected_pipeline_id="123", remote=str(self.remote),
                    scratch_root=self.scratch)
        args.update(overrides)
        return publisher.publish(**args)

    def test_authentication_opt_in_is_only_for_approved_https_and_askpass(self):
        auth = {"GIT_ASKPASS": str(self.root / "askpass"),
                "GITHUB_PUBLIC_TOKEN": "fixture-only"}
        with self.assertRaises(publisher.PublishError):
            self.publish(authentication_environment=auth)
        with self.assertRaises(publisher.PublishError):
            self.publish(remote="https://github.com/mlimarenko/piping.git",
                         authentication_environment={**auth, "GIT_CONFIG_GLOBAL": "foreign"})
        with self.assertRaises(publisher.PublishError):
            self.publish(remote="git@github.com:mlimarenko/piping.git",
                         authentication_environment=auth)
        self.assertEqual(self.refs(), self.initial_refs)

    def refs(self):
        return self.git(self.remote, "for-each-ref", "--format=%(refname) %(objectname)")

    def names(self):
        return self.git(self.remote, "ls-tree", "-r", "--name-only", "main").splitlines()

    def show(self, path):
        return self.git(self.remote, "show", f"main:{path}")

    def select_next(self, version="1.0.1"):
        (self.source / "next").write_text(f"next committed release {version}\n")
        self.commit("feat(PIP-137): Add saved sessions")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection(version, self.sha)
        self.save_selection()

    def test_root_isolated_committed_export_projection_and_rendering(self):
        (self.source / "run.sh").write_text("dirty must not leak\n")
        (self.source / "untracked").write_text("not committed\n")
        result = self.publish()
        self.assertEqual(result["status"], "published")
        self.assertIsNone(result["parent"])
        self.assertEqual(self.git(self.remote, "rev-list", "--count", "main").strip(), "1")
        self.assertEqual(self.git(self.remote, "rev-parse", "1.0.0").strip(), result["public_commit"])
        self.assertEqual(self.show("run.sh"), "#!/bin/sh\necho committed\n")
        names = self.names()
        entries = self.git(self.remote, "ls-tree", "-r", "main")
        self.assertIn("100755 blob", entries)
        self.assertIn("120000 blob", entries)
        self.assertIn(".github/workflows/release-docker-images.yml", names)
        self.assertNotIn(".github/workflows/build.yml", names)
        self.assertNotIn(".github/workflows/desktop.yml", names)
        self.assertIn(".github/CONTRIBUTING.md", names)
        self.assertNotIn("env/images.env", names)
        self.assertNotIn("untracked", names)
        self.assertNotEqual(self.git_status(self.remote, "cat-file", "-e", "main:env/images.env"), 0)
        self.assertEqual(self.show(".github/workflows/release-docker-images.yml"), WORKFLOW)
        self.assertEqual(self.show("current"), "run.sh")
        descriptor = json.loads(self.show("env/public-source.json"))
        self.assertEqual(descriptor, {
            "schema": "piping.public-source/v1",
            "version": "1.0.0",
            "commit": self.sha,
            "pipeline_id": "123",
        })
        installed = self.show("install.sh")
        self.assertEqual(installed, '#!/bin/sh\nPUBLIC_VERSION="1.0.0"\n')
        self.assertEqual(
            self.git(self.remote, "ls-tree", "main", "install.sh").split()[0],
            "100755")
        readme = self.show("README.md")
        self.assertIn("/1.0.0/", readme)
        self.assertNotIn(publisher.VERSION_TOKEN, readme)
        self.assertNotIn(publisher.VERSION_TOKEN, installed)
        self.assertEqual(result["tree"], self.git(self.remote, "rev-parse", "main^{tree}").strip())
        for private_sha in (self.ancestor, self.sha):
            self.assertNotEqual(
                self.git_status(self.remote, "cat-file", "-e", private_sha), 0,
                "private Git object was imported")
        self.assertEqual(list(self.scratch.iterdir()), [])

    def test_append_one_commit_idempotency_and_historical_retry(self):
        first_selection = copy.deepcopy(self.selection_document)
        first_sha = self.sha
        first = self.publish()
        self.assertEqual(self.publish()["status"], "unchanged")
        self.select_next()
        second = self.publish()
        self.assertEqual(second["parent"], first["public_commit"])
        self.assertEqual(self.git(self.remote, "rev-list", "--count", "main").strip(), "2")
        self.assertEqual(self.git(self.remote, "rev-parse", "1.0.1").strip(), second["public_commit"])
        self.assertEqual(
            json.loads(self.git(self.remote, "show", f"{second['public_commit']}:env/public-source.json")),
            {"schema": "piping.public-source/v1", "version": "1.0.1",
             "commit": self.sha, "pipeline_id": "123"})
        before = self.refs()
        self.assertEqual(self.publish()["public_commit"], second["public_commit"])
        self.sha, self.selection_document = first_sha, first_selection
        self.save_selection()
        self.assertEqual(self.publish()["public_commit"], first["public_commit"])
        self.assertEqual(self.refs(), before)

    def test_deterministic_root_metadata(self):
        first = self.publish()
        identity = self.git(self.remote, "show", "-s", "--format=%an <%ae>%n%cn <%ce>", "main").splitlines()
        self.assertEqual(identity, ["mlimarenko <82709956+mlimarenko@users.noreply.github.com>"] * 2)
        other = self.root / "other.git"
        other.mkdir()
        self.git(other, "init", "--quiet", "--bare", "--initial-branch=main")
        second = self.publish(remote=str(other))
        self.assertEqual(first["public_commit"], second["public_commit"])

    def test_changelog_starts_without_dumping_private_history(self):
        self.publish()
        notes = self.show("CHANGELOG.md")
        self.assertEqual(notes, "# Changelog\n\n## 1.0.0\n\n### Changed\n\n- Initial public snapshot.\n")
        self.assertNotIn(self.sha, notes)
        self.assertNotIn("Internal ancestor", notes)
        self.assertEqual(self.git(self.remote, "ls-tree", "main", "CHANGELOG.md").split()[0], "100644")

    def test_changelog_preserves_public_history_without_private_commit_prose(self):
        self.publish()
        previous = self.show("CHANGELOG.md")
        (self.source / "change").write_text("feature\n")
        self.commit("feat(inventory)!: PIP-137: Add device groups for Confidential Client")
        (self.source / "change").write_text("fix\n")
        self.commit("fix(PIP-138): Repair [session reconnect](https://gitlab.piping.space/private) "
                    + "a" * 40 + " pipeline #7096")
        (self.source / "change").write_text("docs\n")
        self.commit("docs(guide): Explain saved groups (PIP-139)")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        second_selection = self.selection("1.0.1", self.sha)
        self.selection_document = copy.deepcopy(second_selection)
        self.save_selection()
        (self.source / "CHANGELOG.md").write_text("dirty private notes\n")
        (self.source / "change").write_text("dirty implementation\n")
        second = self.publish()
        notes = self.show("CHANGELOG.md")
        self.assertTrue(notes.endswith(previous.removeprefix("# Changelog\n\n")))
        self.assertEqual(notes, "# Changelog\n\n## 1.0.1\n\n### Changed\n\n"
                         "- Updated public snapshot.\n\n" + previous.removeprefix("# Changelog\n\n"))
        for private in ("Confidential Client", "Add device groups", "Repair session reconnect",
                        "Explain saved groups", "PIP-", "gitlab", "https://", "7096",
                        "a" * 40, "dirty", self.sha):
            self.assertNotIn(private, notes)
        self.assertEqual(self.show("change"), "docs\n")
        self.assertEqual(self.publish()["status"], "unchanged")
        second_sha = self.sha
        (self.source / "CHANGELOG.md").unlink()
        self.select_next("1.0.2")
        self.publish()
        later = self.show("CHANGELOG.md")
        refs = self.refs()
        self.sha, self.selection_document = second_sha, second_selection
        self.save_selection()
        retry = self.publish()
        self.assertEqual(retry["status"], "unchanged")
        self.assertEqual(retry["public_commit"], second["public_commit"])
        self.assertEqual(self.refs(), refs)
        self.assertEqual(self.show("CHANGELOG.md"), later)
        self.assertEqual(self.git(self.remote, "show", "1.0.1:CHANGELOG.md"), notes)

    def test_changelog_unsafe_source_path_is_refused(self):
        for kind in ("symlink", "directory"):
            with self.subTest(kind=kind):
                target = self.source / "CHANGELOG.md"
                if kind == "symlink":
                    target.symlink_to("README.md")
                else:
                    target.unlink()
                    target.mkdir()
                    (target / "notes").write_text("not a regular file\n")
                self.commit("docs(PIP-137): Unsafe changelog fixture")
                self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
                self.selection_document = self.selection("1.0.0", self.sha)
                self.save_selection()
                with self.assertRaises(publisher.PublishError):
                    self.publish()
                self.assertEqual(self.refs(), self.initial_refs)

    def test_changelog_excludes_merged_private_commit_prose(self):
        self.publish()
        self.git(self.source, "checkout", "--quiet", "-b", "feature")
        (self.source / "feature").write_text("merged feature\n")
        self.commit("feat(inventory): Add merged device labels")
        self.git(self.source, "checkout", "--quiet", "develop")
        self.git(self.source, "merge", "--quiet", "--no-ff", "feature", "-m", "Merge feature branch")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.1", self.sha)
        self.save_selection()
        self.publish()
        notes = self.show("CHANGELOG.md")
        self.assertNotIn("Add merged device labels", notes)
        self.assertNotIn("Merge feature branch", notes)
        self.assertEqual(self.show("feature"), "merged feature\n")

    def replace_prior_changelog(self, content, *, mode="100644", source=None):
        # Build an immutable predecessor fixture using native Git objects, not
        # the publisher's rendering logic. Only fixture refs are replaced here.
        blob = self.git(self.remote, "hash-object", "-w", "--stdin", data=content).strip()
        tree = self.git(self.remote, "mktree", data=f"{mode} blob {blob}\tCHANGELOG.md\n").strip()
        message = ("Release 1.0.0\n\nSource-Commit: " + (source or self.sha)
                   + "\nRelease-Pipeline: 123\nRelease-Manifest-SHA256: " + "b" * 64 + "\n")
        prior = self.git(self.remote, "commit-tree", tree, data=message).strip()
        self.git(self.remote, "update-ref", "refs/heads/main", prior)
        self.git(self.remote, "update-ref", "refs/tags/1.0.0", prior)
        return prior

    def test_changelog_foreign_provenance_refuses_without_refs_mutation(self):
        unrelated = self.git(self.source, "commit-tree", "HEAD^{tree}",
                             data="feat: Unrelated source root\n").strip()
        for source in ("a" * 40, unrelated):
            with self.subTest(source=source):
                self.replace_prior_changelog("# Changelog\n", source=source)
                self.selection_document = self.selection("1.0.1", self.sha)
                self.save_selection()
                before = self.refs()
                with self.assertRaisesRegex(publisher.PublishError, "not an ancestor"):
                    self.publish()
                self.assertEqual(self.refs(), before)

    def test_changelog_history_type_size_and_duplicate_version_are_refused(self):
        cases = [("README.md", "120000"),
                 ("x" * (publisher.MAX_CHANGELOG_BYTES + 1), "100644"),
                 ("# Changelog\n\n## 1.0.1\n\nForeign entry\n", "100644")]
        for content, mode in cases:
            with self.subTest(mode=mode, length=len(content)):
                self.replace_prior_changelog(content, mode=mode)
                self.selection_document = self.selection("1.0.1", self.sha)
                self.save_selection()
                before = self.refs()
                with self.assertRaises(publisher.PublishError):
                    self.publish()
                self.assertEqual(self.refs(), before)

    def test_changelog_preserves_preexisting_published_notes_without_invented_versions(self):
        previous = "# Changelog\n\n## 1.0.0\n\n### Added\n\n- Existing published notes.\n"
        prior = self.replace_prior_changelog(previous)
        self.select_next()
        self.publish()
        notes = self.show("CHANGELOG.md")
        self.assertTrue(notes.endswith(previous.removeprefix("# Changelog\n\n")))
        self.assertIn("- Updated public snapshot.\n", notes)
        self.assertNotIn("Add saved sessions", notes)
        self.assertEqual(self.git(self.remote, "show", f"{prior}:CHANGELOG.md"), previous)

    def test_local_archive_attributes_cannot_change_selected_source(self):
        first = self.publish()
        attributes = self.source / ".git/info/attributes"
        attributes.write_text("run.sh export-ignore\n.github/CONTRIBUTING.md export-ignore\n")
        (self.source / ".gitattributes").write_text("current export-ignore\n")
        other = self.root / "other.git"
        other.mkdir()
        self.git(other, "init", "--quiet", "--bare", "--initial-branch=main")
        second = self.publish(remote=str(other))
        self.assertEqual(first["public_commit"], second["public_commit"])
        self.assertEqual(self.git(other, "show", "main:run.sh"), "#!/bin/sh\necho committed\n")

    def test_sha256_source_exports_into_independent_github_object_format(self):
        self.source = self.root / "private-sha256"
        self.source.mkdir()
        self.git(self.source, "init", "--quiet", "--initial-branch=develop", "--object-format=sha256")
        (self.source / "payload").write_text("SHA-256 source repository\n")
        (self.source / ".github/workflows").mkdir(parents=True)
        (self.source / ".github/workflows/release-docker-images.yml").write_text(WORKFLOW)
        self.commit("Private SHA-256 commit")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.assertEqual(len(self.sha), 64)
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        receipt = self.publish()
        self.assertEqual(receipt["source"], self.sha)
        self.assertEqual(len(receipt["public_commit"]), 40)
        self.assertEqual(self.git(self.remote, "show", "main:payload"), "SHA-256 source repository\n")
        self.select_next()
        self.publish()
        notes = self.show("CHANGELOG.md")
        self.assertIn("- Updated public snapshot.\n", notes)
        self.assertNotIn("Add saved sessions", notes)

    def test_wrong_first_version_is_refused_without_remote_mutation(self):
        for version in ("0.9.0", "1.0.1", "2.0.0"):
            with self.subTest(version=version):
                self.selection_document = self.selection(version, self.sha)
                self.save_selection()
                with self.assertRaisesRegex(publisher.PublishError, "first public release"):
                    self.publish()
                self.assertEqual(self.refs(), "")

    def test_leftover_old_namespace_ref_is_not_adopted(self):
        self.git(self.source, "push", "--quiet", str(self.remote), "HEAD:refs/tags/v1.0.131")
        before = self.refs()
        with self.assertRaisesRegex(publisher.PublishError, "unexpected refs"):
            self.publish()
        self.assertEqual(self.refs(), before)

    def test_noninitial_release_root_is_not_adopted(self):
        tree = self.git(self.remote, "mktree", data="").strip()
        message = (f"Release 1.0.1\n\nSource-Commit: {self.sha}\n"
                   "Release-Pipeline: 123\nRelease-Manifest-SHA256: " + "b" * 64 + "\n")
        root = self.git(self.remote, "commit-tree", tree, data=message).strip()
        self.git(self.remote, "update-ref", "refs/heads/main", root)
        self.git(self.remote, "update-ref", "refs/tags/1.0.1", root)
        before = self.refs()
        with self.assertRaisesRegex(publisher.PublishError, "root must be 1.0.0"):
            self.publish()
        self.assertEqual(self.refs(), before)

    def test_second_reset_and_active_old_namespace_are_refused(self):
        self.publish()
        self.select_next()
        second = self.publish()
        # Native Git creates a forged but otherwise plausible release descendant.
        for version, tag in (("1.0.0", "1.0.0"), ("1.0.2", "v1.0.2")):
            with self.subTest(tag=tag):
                message = (f"Release {version}\n\nSource-Commit: {self.sha}\n"
                           "Release-Pipeline: 123\nRelease-Manifest-SHA256: " + "b" * 64 + "\n")
                forged = self.git(self.remote, "commit-tree", second["tree"],
                                  "-p", second["public_commit"], data=message).strip()
                previous = self.git(self.remote, "rev-parse", tag).strip() if tag == "1.0.0" else None
                self.git(self.remote, "update-ref", "refs/heads/main", forged)
                self.git(self.remote, "update-ref", f"refs/tags/{tag}", forged)
                before = self.refs()
                with self.assertRaises(publisher.PublishError):
                    self.publish()
                self.assertEqual(self.refs(), before)
                self.git(self.remote, "update-ref", "refs/heads/main", second["public_commit"])
                if previous:
                    self.git(self.remote, "update-ref", f"refs/tags/{tag}", previous)
                else:
                    self.git(self.remote, "update-ref", "-d", f"refs/tags/{tag}")

    def test_same_version_reserved_by_another_pipeline_conflicts_without_mutation(self):
        self.publish()
        before = self.refs()
        self.selection_document = self.selection("1.0.0", self.sha, "124", "124")
        self.save_selection()
        with self.assertRaisesRegex(publisher.PublishError, "conflicts"):
            self.publish(expected_pipeline_id="124")
        self.assertEqual(self.refs(), before)

    def test_same_tree_with_different_source_provenance_is_not_idempotent(self):
        self.publish()
        before = self.refs()
        self.git(self.source, "commit", "--quiet", "--allow-empty", "-m", "Private metadata change")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        with self.assertRaisesRegex(publisher.PublishError, "conflicts"):
            self.publish()
        self.assertEqual(self.refs(), before)
        self.select_next(version="1.0.0")
        with self.assertRaisesRegex(publisher.PublishError, "conflicts"):
            self.publish()
        self.assertEqual(self.refs(), before)

    def test_backward_new_version_has_no_mutation(self):
        self.publish()
        self.selection_document = self.selection("2.0.0", self.sha)
        self.save_selection()
        self.publish()
        before = self.refs()
        self.select_next()
        with self.assertRaisesRegex(publisher.PublishError, "advance"):
            self.publish()
        self.assertEqual(self.refs(), before)

    def test_old_private_mirror_rejected_without_mutation(self):
        self.git(self.source, "push", "--force", str(self.remote), "HEAD:refs/heads/main")
        before = self.refs()
        with self.assertRaisesRegex(publisher.PublishError, "non-release"):
            self.publish()
        self.assertEqual(self.refs(), before)

    def test_foreign_refs_and_mismatched_tags_preserved(self):
        first = self.publish()
        for ref in ("refs/heads/foreign", "refs/tags/foreign", "refs/tags/v99.0.0", "refs/tags/99.0.0"):
            with self.subTest(ref=ref):
                self.git(self.remote, "update-ref", ref, first["public_commit"])
                before = self.refs()
                with self.assertRaises(publisher.PublishError):
                    self.publish()
                self.assertEqual(self.refs(), before)
                self.git(self.remote, "update-ref", "-d", ref)
        self.git(self.remote, "update-ref", "-d", "refs/tags/1.0.0")
        before = self.refs()
        with self.assertRaisesRegex(publisher.PublishError, "tag"):
            self.publish()
        self.assertEqual(self.refs(), before)

    def test_merge_and_annotated_release_tag_are_not_adopted(self):
        first = self.publish()
        self.git(self.remote, "tag", "-f", "-a", "1.0.0", first["public_commit"], "-m", "Annotated")
        before = self.refs()
        with self.assertRaisesRegex(publisher.PublishError, "tag"):
            self.publish()
        self.assertEqual(self.refs(), before)
        self.git(self.remote, "update-ref", "refs/tags/1.0.0", first["public_commit"])
        sibling = self.git(self.remote, "commit-tree", first["tree"], data="Other root\n").strip()
        merge = self.git(self.remote, "commit-tree", first["tree"], "-p", first["public_commit"],
                         "-p", sibling, data="Merge private history\n").strip()
        self.git(self.remote, "update-ref", "refs/heads/main", merge)
        before = self.refs()
        with self.assertRaisesRegex(publisher.PublishError, "single-parent"):
            self.publish()
        self.assertEqual(self.refs(), before)

    def test_concurrent_branch_or_tag_change_is_atomic_and_preserves_foreign_refs(self):
        first = self.publish()
        self.select_next()
        real_git = publisher.git
        for race in ("branch", "tag", "foreign"):
            with self.subTest(race=race):
                changed = {}

                def concurrent_git(directory, *args, **kwargs):
                    if args[0] == "push":
                        if race == "branch":
                            commit = self.git(self.remote, "commit-tree", first["tree"], "-p", first["public_commit"],
                                              data="Concurrent operator\n").strip()
                            self.git(self.remote, "update-ref", "refs/heads/main", commit)
                        else:
                            ref = "refs/tags/1.0.1" if race == "tag" else "refs/heads/foreign"
                            self.git(self.remote, "update-ref", ref, first["public_commit"])
                        changed["refs"] = self.refs()
                    return real_git(directory, *args, **kwargs)

                with patch.object(publisher, "git", side_effect=concurrent_git):
                    with self.assertRaises(publisher.PublishError):
                        self.publish()
                if race == "foreign":
                    self.assertEqual(self.git(self.remote, "rev-parse", "refs/heads/foreign").strip(), first["public_commit"])
                else:
                    self.assertEqual(self.refs(), changed["refs"])
                # Restore only fixture refs, outside the publisher.
                self.git(self.remote, "update-ref", "refs/heads/main", first["public_commit"])
                self.git(self.remote, "update-ref", "-d", "refs/tags/1.0.1")
                self.git(self.remote, "update-ref", "-d", "refs/heads/foreign")

    def test_first_publication_absence_leases_reject_branch_and_tag_races(self):
        real_git = publisher.git
        for ref in ("refs/heads/main", "refs/tags/1.0.0"):
            with self.subTest(ref=ref):
                changed = {}

                def concurrent_git(directory, *args, **kwargs):
                    if args[0] == "push":
                        tree = self.git(self.remote, "mktree", data="").strip()
                        foreign = self.git(self.remote, "commit-tree", tree, data="Concurrent root\n").strip()
                        self.git(self.remote, "update-ref", ref, foreign)
                        changed["refs"] = self.refs()
                    return real_git(directory, *args, **kwargs)

                with patch.object(publisher, "git", side_effect=concurrent_git):
                    with self.assertRaises(publisher.PublishError):
                        self.publish()
                self.assertEqual(self.refs(), changed["refs"])
                self.git(self.remote, "update-ref", "-d", ref)

    def test_private_cohort_directory_is_removed(self):
        (self.source / "env/images.env").unlink()
        (self.source / "env/images.env").mkdir()
        (self.source / "env/images.env/digest").write_text("sha256:" + "c" * 64 + "\n")
        self.commit("Private cohort directory")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        self.publish()
        self.assertFalse(any(name.startswith("env/images.env") for name in self.names()))
        self.assertEqual(json.loads(self.show("env/public-source.json"))["commit"], self.sha)

    def test_legacy_private_image_manifest_is_refused(self):
        self.release.write_text(json.dumps({
            "schema": "ci-components.release/v1", "release_id": "release-123-" + self.sha[:12],
            "version": "1.0.0", "commit": self.sha, "pipeline_id": 123, "files": [],
            "images": [],
        }))
        with self.assertRaises((AdapterError, publisher.PublishError)):
            self.publish()
        self.assertEqual(self.refs(), self.initial_refs)

    def test_selection_boundary_rejects_wrong_state_source_and_pipeline_iid(self):
        original = copy.deepcopy(self.selection_document)
        cases = {
            "schema": lambda d: d.update(schema="other/v1"),
            "commit": lambda d: d.update(commit="f" * 40),
            "pipeline-id-type": lambda d: d.update(pipeline_id=123),
            "pipeline-iid-type": lambda d: d.update(pipeline_iid=123),
            "pipeline-iid": lambda d: d.update(pipeline_iid="124"),
            "reservation-not-object": lambda d: d.update(reservation=[]),
            "reservation-missing": lambda d: d["reservation"].pop("tag_reserved"),
            "reservation-git-sha": lambda d: d["reservation"].update(git_sha="f" * 40),
            "reservation-build-id": lambda d: d["reservation"].update(build_id="124"),
            "tag-not-reserved": lambda d: d["reservation"].update(tag_reserved=False),
            "not-candidate": lambda d: d["reservation"].update(is_release_candidate=False),
            "is-prerelease": lambda d: d["reservation"].update(is_prerelease=True),
            "prerelease-version": lambda d: d["reservation"].update(version="1.0.0-rc.1"),
            "core": lambda d: d["reservation"].update(core="1.0.1"),
            "release-line": lambda d: d["reservation"].update(release_line="1.1"),
            "image-tag": lambda d: d["reservation"].update(image_tag="1.0.0_build.1"),
            "git-tag": lambda d: d["reservation"].update(git_tag="release-1.0.0"),
            "channel": lambda d: d["reservation"].update(channel="rc"),
            "source": lambda d: d["reservation"].update(source="tag"),
            "extra-wrapper-field": lambda d: d.update(manifest="private"),
        }
        for name, mutate in cases.items():
            with self.subTest(case=name):
                self.selection_document = copy.deepcopy(original)
                mutate(self.selection_document)
                self.save_selection()
                with self.assertRaises((AdapterError, publisher.PublishError)):
                    self.publish()
                self.assertEqual(self.refs(), self.initial_refs)
        self.selection_document = copy.deepcopy(original)
        self.save_selection()
        with self.assertRaisesRegex((AdapterError, publisher.PublishError), "another commit"):
            self.publish(expected_commit="f" * 40)
        with self.assertRaisesRegex((AdapterError, publisher.PublishError), "another pipeline"):
            self.publish(expected_pipeline_id="124")
        self.assertEqual(self.refs(), self.initial_refs)

    def test_duplicate_deep_and_oversized_json_are_safe_boundary_errors(self):
        self.release.write_text('{"schema":"one","schema":"two"}')
        with self.assertRaises((AdapterError, publisher.PublishError)):
            self.publish()
        self.release.write_text('{"pipeline_id":' + "9" * 5000 + "}")
        with self.assertRaisesRegex((AdapterError, publisher.PublishError), "canonical JSON"):
            self.publish()
        self.release.write_text("[" * 600 + "0" + "]" * 600)
        # Iterative decoders reach schema validation; recursive decoders may
        # refuse at parse time. Both must stay within the safe error boundary.
        with self.assertRaises((AdapterError, publisher.PublishError)):
            self.publish()
        self.release.unlink()
        self.release.symlink_to(self.source / "run.sh")
        with self.assertRaises(AdapterError):
            self.publish()
        self.assertEqual(self.refs(), self.initial_refs)

    def test_remote_and_scratch_boundary(self):
        for remote in ("‹вырезано›github.com/mlimarenko/piping.git",
                       "https://github.com/other/piping.git", "git@github.com:other/piping.git",
                       "file:///somewhere", "relative.git", str(self.source), "ext::command"):
            with self.subTest(remote=remote), self.assertRaises(publisher.PublishError):
                self.publish(remote=remote)
        with self.assertRaises(publisher.PublishError):
            self.publish(scratch_root=Path("relative"))
        for allowed in ("git@github.com:mlimarenko/piping.git", "https://github.com/mlimarenko/piping.git"):
            self.assertEqual(publisher.validate_remote(allowed), allowed)
        self.assertEqual(self.refs(), self.initial_refs)

    def test_missing_public_workflow_is_refused_without_mutation(self):
        (self.source / ".github/workflows/release-docker-images.yml").unlink()
        self.commit("Drop public workflow")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        with self.assertRaisesRegex(publisher.PublishError, "release-docker-images"):
            self.publish()
        self.assertEqual(self.refs(), self.initial_refs)

    def test_token_literal_outside_rendered_consumers_is_preserved(self):
        literal = "api-version: __PIPING_PUBLIC_VERSION__\n"
        (self.source / "docs").mkdir()
        (self.source / "docs/version.txt").write_text(literal)
        self.commit("Non-template version literal")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        self.publish()
        self.assertEqual(self.show("docs/version.txt"), literal)
        self.assertNotIn(publisher.VERSION_TOKEN, self.show("README.md"))
        self.assertNotIn(publisher.VERSION_TOKEN, self.show("install.sh"))

    def test_committed_publication_tooling_literals_do_not_block_publication(self):
        # The real selected source contains this publisher and this test module,
        # both of which spell the template token outside the rendered consumers.
        tooling = {
            "scripts/ci/publish_public_release.py":
                Path(publisher.__file__).resolve().read_text(encoding="utf-8"),
            "tests/ci/test_public_release.py":
                Path(__file__).resolve().read_text(encoding="utf-8"),
        }
        for relative, content in tooling.items():
            target = self.source / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8")
        self.commit("Committed publication tooling")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        result = self.publish()
        self.assertEqual(result["status"], "published")
        for relative, content in tooling.items():
            self.assertEqual(self.show(relative), content)
        self.assertIn(publisher.VERSION_TOKEN, self.show("scripts/ci/publish_public_release.py"))
        self.assertIn('PUBLIC_VERSION="1.0.0"', self.show("install.sh"))
        self.assertIn("/1.0.0/", self.show("README.md"))
        self.assertNotIn(publisher.VERSION_TOKEN, self.show("README.md"))

    def test_optional_templates_do_not_block_publication(self):
        (self.source / "install.sh").unlink()
        (self.source / "README.md").unlink()
        self.commit("No install template")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        self.publish()
        names = self.names()
        self.assertNotIn("install.sh", names)
        self.assertNotIn("README.md", names)
        self.assertIn(".github/workflows/release-docker-images.yml", names)

    def test_unsafe_symlink_is_rejected_before_public_mutation(self):
        (self.source / "escape").symlink_to("../../outside")
        self.commit("Unsafe source link")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        with self.assertRaisesRegex(publisher.PublishError, "symlink"):
            self.publish()
        self.assertEqual(self.refs(), self.initial_refs)

    def test_chained_symlink_escape_is_rejected(self):
        (self.source / "alias").symlink_to(".")
        (self.source / "dir").mkdir()
        (self.source / "dir/escape").symlink_to("../alias/../outside")
        self.commit("Chained source link")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        with self.assertRaisesRegex(publisher.PublishError, "symlink chain"):
            self.publish()
        self.assertEqual(self.refs(), self.initial_refs)

    def test_input_paths_are_regular_and_output_paths_are_not_followed(self):
        (self.source / "env/public-source.json").symlink_to("../run.sh")
        self.commit("Symlinked descriptor")
        self.sha = self.git(self.source, "rev-parse", "HEAD").strip()
        self.selection_document = self.selection("1.0.0", self.sha)
        self.save_selection()
        with self.assertRaisesRegex(publisher.PublishError, "regular file"):
            self.publish()
        self.assertEqual(self.refs(), self.initial_refs)

    def test_cli_emits_one_json_receipt_and_safe_error(self):
        script = Path(publisher.__file__).resolve()
        command = [sys.executable, str(script), "--source", str(self.source),
                   "--release", str(self.release), "--expected-commit", self.sha,
                   "--expected-pipeline-id", "123", "--remote", str(self.remote),
                   "--scratch-root", str(self.scratch)]
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["status"], "published")
        self.assertEqual(result.stderr, "")
        command[command.index("--remote") + 1] = "‹вырезано›github.com/mlimarenko/piping.git"
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("do-not-print", result.stderr)
        self.assertEqual(result.stdout, "")


class PublicSelectionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="pip136-select-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sha = "a" * 40
        self.reservation = self.native_reservation()
        self.reservation_path = self.root / "version.json"
        self.reservation_path.write_text(json.dumps(self.reservation))
        self.output = self.root / "public-selection.json"

    def native_reservation(self):
        return {
            "version": "1.2.3", "core": "1.2.3", "major": "1", "minor": "2", "patch": "3",
            "prerelease": "", "build_metadata": "", "channel": "stable",
            "source": "release", "release_line": "1.2", "release_bump": "patch",
            "git_tag": "1.2.3", "image_tag": "1.2.3",
            "is_release": False, "is_release_candidate": True, "is_prerelease": False,
            "build_id": "456", "git_sha": self.sha, "tag_reserved": True,
        }

    def run_selection(self, *, commit=None, pipeline_id="789", pipeline_iid="456",
                      reservation=None, output=None):
        command = [sys.executable, str(Path(selector.__file__).resolve()),
                   "--reservation", str(reservation or self.reservation_path),
                   "--commit", commit or self.sha, "--pipeline-id", pipeline_id,
                   "--pipeline-iid", pipeline_iid,
                   "--output", str(output or self.output)]
        return subprocess.run(command, capture_output=True, text=True, timeout=30)

    def test_writes_exact_wrapper_the_publisher_accepts(self):
        result = self.run_selection()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["version"], "1.2.3")
        document = json.loads(self.output.read_text())
        self.assertEqual(document, {
            "schema": "piping.public-selection/v1", "commit": self.sha,
            "pipeline_id": "789", "pipeline_iid": "456",
            "reservation": self.reservation,
        })
        selected = publisher.validate_selection(
            document, expected_commit=self.sha, expected_pipeline_id="789")
        self.assertEqual(selected, {"version": "1.2.3", "commit": self.sha,
                                    "pipeline_iid": "456", "pipeline_id": "789"})

    def test_refuses_wrong_reservation_state_source_and_identity(self):
        mutations = {
            "tag-not-reserved": lambda r: r.update(tag_reserved=False),
            "not-candidate": lambda r: r.update(is_release_candidate=False),
            "prerelease": lambda r: r.update(is_prerelease=True, version="1.2.3-rc.1",
                                             prerelease="rc.1"),
            "unstable-version": lambda r: r.update(version="1.2.3-rc.1", prerelease="rc.1"),
            "git-sha": lambda r: r.update(git_sha="b" * 40),
            "build-id": lambda r: r.update(build_id="999"),
            "image-tag": lambda r: r.update(image_tag="1.2.3_build.1"),
            "source": lambda r: r.update(source="tag"),
            "not-object": lambda r: [],
        }
        original = copy.deepcopy(self.reservation)
        for name, mutate in mutations.items():
            with self.subTest(case=name):
                reservation = copy.deepcopy(original)
                mutation = mutate(reservation)
                self.reservation_path.write_text(
                    json.dumps(mutation if mutation is not None else reservation))
                result = self.run_selection()
                self.assertEqual(result.returncode, 1)
                self.assertIn("public selection refused:", result.stderr)
                self.assertEqual(result.stdout, "")
                self.assertFalse(self.output.exists())
        self.reservation_path.write_text(json.dumps(original))
        for name, kwargs in {
            "commit": {"commit": "c" * 40},
            "pipeline-iid": {"pipeline_iid": "999"},
            "pipeline-id": {"pipeline_id": "007"},
        }.items():
            with self.subTest(case=name):
                result = self.run_selection(**kwargs)
                self.assertEqual(result.returncode, 1)
                self.assertFalse(self.output.exists())

    def test_duplicate_and_oversized_reservation_are_safe_errors(self):
        self.reservation_path.write_text('{"version":"1.0.0","version":"1.0.0"}')
        result = self.run_selection()
        self.assertEqual(result.returncode, 1)
        self.assertIn("canonical JSON", result.stderr)
        self.assertFalse(self.output.exists())
        self.reservation_path.write_text('{"pipeline":' + "9" * 5000 + "}")
        result = self.run_selection()
        self.assertEqual(result.returncode, 1)
        self.assertIn("canonical JSON", result.stderr)
        self.assertFalse(self.output.exists())

    def test_output_symlink_is_refused(self):
        victim = self.root / "victim"
        victim.write_text("keep\n")
        self.output.symlink_to(victim)
        result = self.run_selection()
        self.assertEqual(result.returncode, 1)
        self.assertIn("regular non-symlink", result.stderr)
        self.assertEqual(victim.read_text(), "keep\n")


if __name__ == "__main__":
    unittest.main()
