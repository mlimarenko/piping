#!/usr/bin/env python3
"""Publish one selected release as an isolated, Git-native public snapshot."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tarfile
import tempfile

if __package__:
    from .apply_release_bundle import (
        AdapterError, MAX_CONTRACT_BYTES, read_regular_text,
    )
    from .select_public_release import parse_json_document, validate_selection
else:
    from apply_release_bundle import (
        AdapterError, MAX_CONTRACT_BYTES, read_regular_text,
    )
    from select_public_release import parse_json_document, validate_selection

STABLE = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
MESSAGE = re.compile(
    rf"Release (?P<version>{STABLE})\n\n"
    r"Source-Commit: (?P<source>[0-9a-f]{40}|[0-9a-f]{64})\n"
    r"Release-Pipeline: (?P<pipeline>[1-9][0-9]*)\n"
    r"Release-Manifest-SHA256: (?P<manifest>[0-9a-f]{64})\n"
)
# The public snapshot keeps exactly one workflow: the post-snapshot image
# publisher. Every other private workflow and the private image cohort are
# removed, so no GitLab-only automation or relabelled image digest leaks.
PUBLIC_WORKFLOW = (".github", "workflows", "release-docker-images.yml")
IMAGE_COHORT = ("env", "images.env")
PUBLIC_SOURCE = ("env", "public-source.json")
PUBLIC_SOURCE_SCHEMA = "piping.public-source/v1"
VERSION_TOKEN = "__PIPING_PUBLIC_VERSION__"
RENDERED_TEMPLATES = (("install.sh",), ("README.md",))
# Only these user/runtime consumers are version templates. Tooling and tests in
# the selected source may legitimately spell the token literal.
RENDERED_TEMPLATE_PATHS = tuple("/".join(relative) for relative in RENDERED_TEMPLATES)
TIMEOUT = 120
MAX_CHANGELOG_BYTES = 1024 * 1024
MAX_CHANGELOG_COMMITS = 4096
INITIAL_VERSION = "1.0.0"
CONVENTIONAL_SUBJECT = re.compile(
    r"(?P<type>feat|fix|perf|refactor|docs|build|ci|chore|style|test|revert)"
    r"(?:\([^()\n]*\))?!?:\s*(?P<subject>.+)")


class PublishError(ValueError):
    """Publication was refused without exposing subprocess diagnostics."""


def run_git(directory: Path, *arguments: str, data: bytes | None = None,
            output=None, environment: dict[str, str] | None = None,
            allowed: tuple[int, ...] = (0,)) -> subprocess.CompletedProcess:
    # Do not inherit object stores, alternate indexes, trace destinations, URL
    # rewrites, hooks or credential-helper configuration from the operator.
    env = {key: value for key, value in os.environ.items()
           if not key.startswith("GIT_") and key != "GITHUB_PUBLIC_TOKEN"}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_TERMINAL_PROMPT="0", GIT_NO_REPLACE_OBJECTS="1", LC_ALL="C")
    if environment:
        env.update(environment)
    try:
        result = subprocess.run(
            ["git", "-c", f"core.hooksPath={os.devnull}",
             "-c", "protocol.ext.allow=never", "-c", "core.attributesFile=" + os.devnull,
             "-c", "credential.helper=", "-c", "http.followRedirects=false",
             "-C", str(directory), *arguments],
            input=data, stdout=output if output is not None else subprocess.PIPE,
            stderr=subprocess.PIPE, env=env, timeout=TIMEOUT, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PublishError("Git command unavailable or timed out") from error
    if result.returncode not in allowed:
        # Git diagnostics can contain remote credentials, hook output or paths.
        raise PublishError("Git command failed; publication was not confirmed")
    return result


def git(directory: Path, *arguments: str, data: bytes | None = None,
        output=None, environment: dict[str, str] | None = None) -> bytes:
    return run_git(directory, *arguments, data=data, output=output,
                   environment=environment).stdout or b""


def validate_remote(remote: str) -> str:
    if remote in {
        "git@github.com:mlimarenko/piping.git", "git@github.com:mlimarenko/piping",
        "ssh://git@github.com/mlimarenko/piping.git", "ssh://git@github.com/mlimarenko/piping",
        "https://github.com/mlimarenko/piping.git", "https://github.com/mlimarenko/piping",
    }:
        return remote
    path = Path(remote)
    if not path.is_absolute() or not path.is_dir():
        raise PublishError("remote must be the approved GitHub repository or an absolute local bare repository")
    path = path.resolve()
    if git(path, "rev-parse", "--is-bare-repository").strip() != b"true":
        raise PublishError("local remote must be bare")
    return str(path)


def advertised(directory: Path, remote: str, *,
               environment: dict[str, str] | None = None) -> dict[str, str]:
    refs: dict[str, str] = {}
    for line in git(directory, "ls-remote", "--refs", remote,
                    environment=environment).decode("ascii").splitlines():
        oid, ref = line.split("\t")
        if ref != "refs/heads/main" and re.fullmatch(rf"refs/tags/{STABLE}", ref) is None:
            raise PublishError("public remote has unexpected refs")
        refs[ref] = oid
    if refs and "refs/heads/main" not in refs:
        raise PublishError("public tags exist without main")
    return refs


def version_tuple(version: str) -> tuple[tuple[int, str], ...]:
    # Stable components have no leading zeroes. Compare without converting an
    # untrusted, potentially very long decimal string to a Python integer.
    return tuple((len(component), component) for component in version.split("."))


def validate_history(directory: Path, refs: dict[str, str]) -> dict[str, tuple[str, str, str, str]]:
    """Index one bare-tag release chain rooted at the initial public snapshot."""
    if not refs:
        return {}
    history = {}
    previous_version = None
    for line in git(directory, "rev-list", "--parents", refs["refs/heads/main"]).decode().splitlines():
        fields = line.split()
        if len(fields) > 2:
            raise PublishError("public history is not a single-parent release chain")
        commit = fields[0]
        body = git(directory, "show", "-s", "--format=%B", commit).decode().rstrip("\n") + "\n"
        match = MESSAGE.fullmatch(body)
        if match is None:
            raise PublishError("public history contains a non-release commit")
        version = match["version"]
        current = version_tuple(version)
        if previous_version is not None and current >= previous_version:
            raise PublishError("public release history is not strictly increasing")
        previous_version = current
        # Lightweight tags are canonical; annotated tags are not silently peeled.
        if refs.get(f"refs/tags/{version}") != commit:
            raise PublishError("public release tag does not match its commit")
        tree = git(directory, "rev-parse", f"{commit}^{{tree}}").decode().strip()
        history[version] = (commit, tree, fields[1] if len(fields) == 2 else "", body)
        if len(fields) == 1 and version != INITIAL_VERSION:
            raise PublishError("public release root must be 1.0.0")
    if len(refs) != len(history) + 1:
        raise PublishError("public remote contains unrelated release tags")
    return history


def export_source(source: Path, commit: str, directory: Path, archive: Path) -> None:
    # Submodules cannot be represented by an independent source-only snapshot.
    entries = git(source, "ls-tree", "-rz", commit).split(b"\0")
    if any(entry.startswith(b"160000 ") for entry in entries):
        raise PublishError("release source contains a submodule")
    # Native archive honors $GIT_DIR/info/attributes, even for a commit ID.
    # Use an isolated archive-only repository, reading the source object store
    # in place. It is separate from the outgoing snapshot's repository, and no
    # private commit objects or repository-local attributes are copied there.
    archive_repository = archive.parent / "archive.git"
    archive_repository.mkdir()
    object_format = git(source, "rev-parse", "--show-object-format").decode().strip()
    if object_format not in {"sha1", "sha256"}:
        raise PublishError("unsupported source object format")
    git(archive_repository, "init", "--quiet", "--bare", "--template=", f"--object-format={object_format}")
    object_directory = git(source, "rev-parse", "--path-format=absolute", "--git-path", "objects").decode().rstrip("\n")
    with archive.open("wb") as output:
        git(archive_repository, "archive", "--format=tar", commit, output=output,
            environment={"GIT_OBJECT_DIRECTORY": object_directory})
    with tarfile.open(archive) as bundle:
        symlinks = []
        for member in bundle:
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or any(p.lower() == ".git" for p in path.parts):
                raise PublishError("unsafe path in release archive")
            if tuple(path.parts[:2]) == IMAGE_COHORT:
                # Includes a directory or link named env/images.env, not only
                # the file, so no private cohort entry can survive.
                continue
            if path.parts[:2] == PUBLIC_WORKFLOW[:2]:
                # Exactly one workflow survives; every other private workflow
                # entry, directory or link is dropped rather than committed.
                if tuple(path.parts) != PUBLIC_WORKFLOW or not member.isfile():
                    continue
            target = directory.joinpath(*path.parts)
            if any(parent.is_symlink() for parent in target.parents if parent != directory.parent):
                raise PublishError("archive path traverses a symlink")
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            if member.issym():
                link = PurePosixPath(member.linkname)
                # Preserve relative symlinks only when their lexical destination
                # stays inside the snapshot. Never follow them while exporting.
                depth = len(path.parent.parts)
                if link.is_absolute():
                    raise PublishError("absolute symlink in release source")
                for part in link.parts:
                    depth += -1 if part == ".." else 1 if part != "." else 0
                    if depth < 0 or part.lower() == ".git":
                        raise PublishError("escaping symlink in release source")
                target.symlink_to(member.linkname)
                symlinks.append(target)
            elif member.isfile():
                content = bundle.extractfile(member)
                if content is None:
                    raise PublishError("unreadable release archive member")
                with target.open("xb") as output:
                    while chunk := content.read(1024 * 1024):
                        output.write(chunk)
                target.chmod(0o755 if member.mode & 0o111 else 0o644)
            else:
                raise PublishError("unsupported member in release archive")
        for link in symlinks:
            try:
                link.resolve().relative_to(directory.resolve())
            except (ValueError, RuntimeError) as error:
                raise PublishError("unsafe symlink chain in release source") from error


def snapshot_target(directory: Path, relative: tuple[str, ...]) -> Path:
    target = directory.joinpath(*relative)
    try:
        parents = target.relative_to(directory).parent.parts
    except ValueError as error:
        raise PublishError("public snapshot output escapes the snapshot") from error
    current = directory
    for part in parents:
        current = current / part
        if current.is_symlink():
            raise PublishError("public snapshot output path is a symlink")
        if current.exists() and not current.is_dir():
            raise PublishError("public snapshot output path is not a directory")
    if target.is_symlink() or (target.exists() and not target.is_file()):
        raise PublishError("public snapshot output path is not a regular file")
    return target


def render_template(directory: Path, relative: tuple[str, ...], version: str) -> None:
    target = snapshot_target(directory, relative)
    if not target.exists():
        return
    try:
        content = target.read_text(encoding="utf-8")
    except UnicodeDecodeError as error:
        raise PublishError("public template is not UTF-8") from error
    if VERSION_TOKEN in content:
        target.write_text(content.replace(VERSION_TOKEN, version), encoding="utf-8")


def write_public_source(directory: Path, descriptor: dict[str, str]) -> None:
    target = snapshot_target(directory, PUBLIC_SOURCE)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        json.dumps(descriptor, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8")
    target.chmod(0o644)


def preceding_changelog(directory: Path, parent: str) -> str:
    if not parent:
        return ""
    entry = git(directory, "ls-tree", parent, "--", "CHANGELOG.md").decode().strip()
    if not entry:
        return ""
    if entry.split()[0:2] not in (["100644", "blob"], ["100755", "blob"]):
        raise PublishError("preceding public changelog is not a regular file")
    size = int(git(directory, "cat-file", "-s", f"{parent}:CHANGELOG.md"))
    if size > MAX_CHANGELOG_BYTES:
        raise PublishError("preceding public changelog exceeds size limit")
    try:
        return git(directory, "show", f"{parent}:CHANGELOG.md").decode("utf-8")
    except UnicodeDecodeError:
        raise PublishError("preceding public changelog is not UTF-8") from None


def changelog_subject(subject: str) -> str:
    # Subjects are the only private commit metadata eligible for publication.
    # Remove references, not their linked prose; never emit URLs or identifiers.
    subject = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", subject)
    subject = re.sub(r"(?:[a-zA-Z][a-zA-Z0-9+.-]*://|git@)\S+", "", subject)
    subject = re.sub(r"\b[A-Z][A-Z0-9]*-[0-9]+\b", "", subject)
    subject = re.sub(r"\b[0-9a-fA-F]{40,64}\b", "", subject)
    subject = re.sub(r"\b[\w.+-]+@[\w.-]+\.[A-Za-z]+\b", "", subject)
    subject = re.sub(r"\b(?:pipeline|пайплайн)\s*#?[0-9]+\b", "", subject, flags=re.I)
    subject = re.sub(r"\(\s*\)|\[\s*\]", "", subject)
    subject = " ".join(subject.split()).strip(" :;,-")
    # Treat the result as plain Markdown text, never a link, heading or HTML.
    return re.sub(r"([\\`*_\[\]<>#])", r"\\\1", subject)


def write_changelog(source: Path, commit: str, directory: Path, parent: str,
                    version: str, notes_path: Path) -> None:
    target = snapshot_target(directory, ("CHANGELOG.md",))
    previous = preceding_changelog(directory, parent)
    if re.search(rf"^##\s+\[?{re.escape(version)}\]?(?:\s|$)", previous, flags=re.M):
        raise PublishError("preceding public changelog already contains the selected version")
    groups = {"Added": [], "Fixed": [], "Changed": []}
    if parent:
        body = git(directory, "show", "-s", "--format=%B", parent).decode().rstrip("\n") + "\n"
        provenance = MESSAGE.fullmatch(body)
        if provenance is None:
            raise PublishError("preceding public snapshot has invalid provenance")
        previous_source = provenance["source"]
        # Check in the private object store only. No private commits are fetched
        # into the outgoing SHA-1 repository, including for SHA-256 sources.
        if run_git(source, "merge-base", "--is-ancestor", previous_source, commit,
                   allowed=(0, 1, 128)).returncode != 0:
            raise PublishError("preceding public source is not an ancestor of the selected source")
        source_range = f"{previous_source}..{commit}"
        count = int(git(source, "rev-list", "--count", source_range))
        if count > MAX_CHANGELOG_COMMITS:
            raise PublishError("public changelog source range exceeds commit limit")
        with notes_path.open("xb") as stream:
            git(source, "log", "--topo-order", "--reverse", "--format=%s", source_range, output=stream)
        if notes_path.stat().st_size > MAX_CHANGELOG_BYTES:
            raise PublishError("public changelog source notes exceed size limit")
        try:
            subjects = notes_path.read_text(encoding="utf-8").splitlines()
        except UnicodeDecodeError:
            raise PublishError("public changelog subjects are not UTF-8") from None
        for subject in subjects:
            match = CONVENTIONAL_SUBJECT.fullmatch(subject)
            if match is None:
                continue
            note = changelog_subject(match["subject"])
            category = {"feat": "Added", "fix": "Fixed", "revert": "Fixed"}.get(match["type"], "Changed")
            if note and note not in groups[category]:
                groups[category].append(note)
    if not any(groups.values()):
        groups["Changed"].append("Initial public snapshot." if not parent else "Updated public snapshot.")
    section = f"## {version}\n"
    for category, notes in groups.items():
        if notes:
            section += f"\n### {category}\n\n" + "".join(f"- {note}\n" for note in notes)
    # Keep published history verbatim. A missing predecessor file starts here;
    # it is not evidence from which to fabricate older release notes.
    content = "# Changelog\n\n" + section
    if previous:
        prior_sections = previous.removeprefix("# Changelog\n\n")
        content += "\n" + prior_sections
    if len(content.encode("utf-8")) > MAX_CHANGELOG_BYTES:
        raise PublishError("generated public changelog exceeds size limit")
    target.write_text(content, encoding="utf-8")
    target.chmod(0o644)


def publish(*, source: Path, release: Path, expected_commit: str,
            expected_pipeline_id: str, remote: str, scratch_root: Path,
            authentication_environment: dict[str, str] | None = None) -> dict[str, str | None]:
    # This opt-in is only for the CI HTTPS transport, never local Git commands.
    if authentication_environment is not None:
        if (remote != "https://github.com/mlimarenko/piping.git"
                or set(authentication_environment) != {"GIT_ASKPASS", "GITHUB_PUBLIC_TOKEN"}
                or not authentication_environment["GITHUB_PUBLIC_TOKEN"]
                or not Path(authentication_environment["GIT_ASKPASS"]).is_absolute()):
            raise PublishError("invalid public Git authentication environment")
    # Read once, then validate those same immutable bytes as the canonical
    # public selection. The selection file may be replaced by another process.
    raw = read_regular_text(release, maximum_bytes=MAX_CONTRACT_BYTES, label="public selection")
    if not scratch_root.is_absolute() or not scratch_root.is_dir():
        raise PublishError("scratch root must be an existing absolute directory")
    document = parse_json_document(raw, label="public selection")
    selected = validate_selection(
        document, expected_commit=expected_commit,
        expected_pipeline_id=expected_pipeline_id)
    version = selected["version"]
    selection_hash = hashlib.sha256(json.dumps(
        document, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()).hexdigest()
    remote = validate_remote(remote)
    with tempfile.TemporaryDirectory(prefix="public-release-", dir=scratch_root) as temporary:
        private = Path(temporary)
        resolved = git(source, "rev-parse", "--verify", f"{expected_commit}^{{commit}}").decode().strip()
        if resolved != expected_commit:
            raise PublishError("source commit is not the selected full commit")
        timestamp = git(source, "show", "-s", "--format=%ct", expected_commit).decode().strip()
        if not timestamp.isdecimal():
            raise PublishError("invalid source commit timestamp")
        projected = private / "snapshot"
        projected.mkdir()
        export_source(source, expected_commit, projected, private / "source.tar")
        workflow = projected.joinpath(*PUBLIC_WORKFLOW)
        if workflow.is_symlink() or not workflow.is_file():
            raise PublishError(
                "selected source must contain .github/workflows/release-docker-images.yml")
        write_public_source(projected, {
            "schema": PUBLIC_SOURCE_SCHEMA,
            "version": version,
            "commit": expected_commit,
            "pipeline_id": selected["pipeline_id"],
        })
        for relative in RENDERED_TEMPLATES:
            render_template(projected, relative, version)
        # git init in an exported tree, never clone/copy/link the private .git.
        git(projected, "init", "--quiet", "--initial-branch=main", "--template=", "--object-format=sha1")
        refs = advertised(projected, remote, environment=authentication_environment)
        if refs:
            git(projected, "fetch", "--quiet", "--no-tags", "--update-head-ok", remote,
                 *(f"{ref}:{ref}" for ref in refs), environment=authentication_environment)
            actual = dict((ref, oid) for oid, ref in (
                line.split() for line in git(projected, "show-ref").decode().splitlines()))
            if actual != refs:
                raise PublishError("public refs changed during fetch")
        history = validate_history(projected, refs)
        # A historical retry must reconstruct that version from its own parent,
        # not incorporate changelog entries from today's newer public HEAD.
        parent = history[version][2] if version in history else refs.get("refs/heads/main", "")
        write_changelog(source, expected_commit, projected, parent, version, private / "notes")
        git(projected, "add", "--all", "--force")
        if run_git(projected, "grep", "--cached", "-a", "-l", "-F", "-e",
                   VERSION_TOKEN, "--", *RENDERED_TEMPLATE_PATHS,
                   allowed=(0, 1)).returncode == 0:
            raise PublishError("public snapshot contains an unrendered release placeholder")
        tree = git(projected, "write-tree").decode().strip()
        message = (f"Release {version}\n\nSource-Commit: {expected_commit}\n"
                   f"Release-Pipeline: {selected['pipeline_id']}\n"
                   f"Release-Manifest-SHA256: {selection_hash}\n")
        if version in history:
            commit, old_tree, parent, old_message = history[version]
            if (old_tree, old_message) != (tree, message):
                raise PublishError("existing public version conflicts with selected release")
            if advertised(projected, remote, environment=authentication_environment) != refs:
                raise PublishError("public refs changed during validation")
            status = "unchanged"
        else:
            if not history and version != INITIAL_VERSION:
                raise PublishError("first public release must be 1.0.0")
            if history and version_tuple(version) <= max(map(version_tuple, history)):
                raise PublishError("new public version must advance main")
            parent = refs.get("refs/heads/main", "")
            metadata = {"GIT_AUTHOR_NAME": "mlimarenko", "GIT_AUTHOR_EMAIL": "82709956+mlimarenko@users.noreply.github.com",
                        "GIT_COMMITTER_NAME": "mlimarenko", "GIT_COMMITTER_EMAIL": "82709956+mlimarenko@users.noreply.github.com",
                        "GIT_AUTHOR_DATE": f"@{timestamp} +0000", "GIT_COMMITTER_DATE": f"@{timestamp} +0000"}
            commit = git(projected, "commit-tree", tree, *( ["-p", parent] if parent else []),
                         data=message.encode(), environment=metadata).decode().strip()
            if advertised(projected, remote, environment=authentication_environment) != refs:
                raise PublishError("public refs changed during validation")
            # Exact leases protect even a fast-forward race. There is no blanket
            # force, and the only proposed branch update is a validated child.
            git(projected, "push", "--quiet", "--atomic",
                f"--force-with-lease=refs/heads/main:{parent}",
                f"--force-with-lease=refs/tags/{version}:", remote,
                 f"{commit}:refs/heads/main", f"{commit}:refs/tags/{version}",
                 environment=authentication_environment)
            # Branch/tag leases cannot lock the entire remote ref namespace.
            # Refuse a success receipt if an unrelated ref appeared at push;
            # never roll back or delete another writer's refs to reconcile it.
            expected_refs = {**refs, "refs/heads/main": commit, f"refs/tags/{version}": commit}
            if advertised(projected, remote, environment=authentication_environment) != expected_refs:
                raise PublishError("public refs changed during publication; reconcile before retry")
            status = "published"
        return {"version": version, "source": expected_commit, "public_commit": commit,
                "tree": tree, "parent": parent or None, "status": status}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--expected-commit", required=True)
    parser.add_argument("--expected-pipeline-id", required=True)
    parser.add_argument("--remote", required=True)
    parser.add_argument("--scratch-root", type=Path, required=True)
    args = parser.parse_args()
    try:
        receipt = publish(**vars(args))
    except (AdapterError, PublishError, OSError, UnicodeError, tarfile.TarError) as error:
        # Boundary validators have safe fixed diagnostics; filesystem errors may
        # include operator-controlled paths, so do not print those exceptions.
        detail = str(error) if isinstance(error, (AdapterError, PublishError)) else "invalid local input"
        print(f"public release refused: {detail}", file=sys.stderr)
        return 1
    print(json.dumps(receipt, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
