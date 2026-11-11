#!/usr/bin/env python3
"""Publish the selected public snapshot and its stable GitHub Release once."""

from __future__ import annotations

import argparse
import base64
import http.client
import json
import os
from pathlib import Path
import re
import sys
import tempfile
import urllib.error

if __package__:
    from . import publish_public_release as publisher
    from ..public_release import Network
else:
    import publish_public_release as publisher
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from public_release import Network

REMOTE = "https://github.com/mlimarenko/piping.git"
REPOSITORY = "mlimarenko/piping"
ACTOR_ID = 82709956


class ReleaseError(ValueError):
    """A fixed diagnostic safe for CI output."""


class GitHub(Network):
    """Reuse the canonical HTTPS and redirect boundary without global GH_TOKEN."""

    def __init__(self, token: str):
        super().__init__()
        self.token = token

    def request(self, url, *, headers=None, **kwargs):
        headers = dict(headers or {})
        headers["Authorization"] = "Bearer " + self.token
        headers["Accept"] = "application/vnd.github+json"
        headers["X-GitHub-Api-Version"] = "2022-11-28"
        if kwargs.get("data") is not None:
            headers["Content-Type"] = "application/json"
        return super().request(url, headers=headers, **kwargs)


def api(client: GitHub, path: str, *, data=None, missing=False, method=None):
    try:
        return client.github(path, data=data, method=method or ("POST" if data is not None else "GET"))
    except urllib.error.HTTPError as error:
        status = error.code
        error.close()
        if missing and status == 404:
            return None
        raise ReleaseError(f"GitHub request failed (HTTP {status}); rerun to reconcile") from None
    except (OSError, ValueError, RecursionError, http.client.HTTPException):
        raise ReleaseError("GitHub request not confirmed; rerun to reconcile") from None


def authorize(client: GitHub) -> None:
    try:
        actor = publisher.parse_json_document(
            client.request("https://api.github.com/user"), label="GitHub actor")
    except (OSError, ValueError, RecursionError, http.client.HTTPException):
        raise ReleaseError("GitHub actor verification failed") from None
    if (not isinstance(actor, dict) or actor.get("login") != "mlimarenko"
            or type(actor.get("id")) is not int or actor["id"] != ACTOR_ID):
        raise ReleaseError("GitHub token must belong to the approved public actor")
    repository = api(client, "")
    if (not isinstance(repository, dict) or repository.get("full_name") != REPOSITORY
            or repository.get("html_url") != "https://github.com/" + REPOSITORY
            or not isinstance(repository.get("owner"), dict)
            or repository["owner"].get("login") != "mlimarenko"
            or repository["owner"].get("id") != ACTOR_ID
            or not isinstance(repository.get("permissions"), dict)
            or repository["permissions"].get("push") is not True):
        raise ReleaseError("GitHub repository identity or push permission mismatch")


def verify_tag(client: GitHub, tag: str, commit: str) -> None:
    actual = api(client, "git/ref/tags/" + tag)
    if (not isinstance(actual, dict) or actual.get("ref") != "refs/tags/" + tag
            or not isinstance(actual.get("object"), dict)
            or actual["object"].get("type") != "commit"
            or actual["object"].get("sha") != commit):
        raise ReleaseError("GitHub lightweight tag differs from the published snapshot")


def verify_release(value, tag: str) -> None:
    if (not isinstance(value, dict) or value.get("tag_name") != tag
            or value.get("name") != tag
            or value.get("draft") is not False or value.get("prerelease") is not False
            or type(value.get("id")) is not int or value["id"] <= 0
            or not isinstance(value.get("author"), dict)
            or value["author"].get("login") != "mlimarenko"
            or value["author"].get("id") != ACTOR_ID
            or value.get("html_url") != f"https://github.com/{REPOSITORY}/releases/tag/{tag}"):
        raise ReleaseError("existing or created GitHub Release conflicts with the selection")


def confirm_latest(client: GitHub, release: dict, tag: str) -> None:
    latest = api(client, "releases/latest", missing=True)
    if latest is not None:
        latest_tag = latest.get("tag_name") if isinstance(latest, dict) else None
        if not isinstance(latest_tag, str) or re.fullmatch(publisher.STABLE, latest_tag) is None:
            raise ReleaseError("GitHub latest stable release is invalid")
        verify_release(latest, latest_tag)
        # Reconciliation of an older release must not roll back Latest.
        if publisher.version_tuple(latest_tag) > publisher.version_tuple(tag):
            return
        if latest.get("id") == release["id"]:
            return
    updated = api(client, "releases/" + str(release["id"]),
                  data=json.dumps({"make_latest": "true"}).encode(), method="PATCH")
    verify_release(updated, tag)
    latest = api(client, "releases/latest")
    verify_release(latest, tag)
    if latest["id"] != release["id"]:
        raise ReleaseError("GitHub latest stable release was not confirmed")


def ensure_release(client: GitHub, receipt: dict) -> tuple[dict, str]:
    tag = receipt["version"]
    # The real lightweight tag is authoritative, not target_commitish (which
    # GitHub can retain as a branch name even for a correctly targeted release).
    verify_tag(client, tag, receipt["public_commit"])
    release = api(client, "releases/tags/" + tag, missing=True)
    status = "unchanged"
    if release is None:
        document = api(client, f"contents/CHANGELOG.md?ref={receipt['public_commit']}")
        if not isinstance(document, dict) or document.get("encoding") != "base64":
            raise ReleaseError("published changelog could not be read")
        text = base64.b64decode(document["content"]).decode("utf-8")
        notes = publisher.changelog_section(text, tag)
        release = api(client, "releases", data=json.dumps({
            "tag_name": tag, "target_commitish": receipt["public_commit"],
            "name": tag, "body": notes, "draft": False, "prerelease": False,
            "generate_release_notes": False, "make_latest": "true",
        }).encode())
        status = "published"
        if release.get("body") != notes:
            raise ReleaseError("GitHub Release notes were not confirmed")
    verify_release(release, tag)
    verify_tag(client, tag, receipt["public_commit"])
    confirm_latest(client, release, tag)
    verify_tag(client, tag, receipt["public_commit"])
    return release, status


def output_target(path: Path) -> Path:
    path = path.absolute()
    return publisher.snapshot_target(Path(path.anchor), path.parts[1:])


def write_receipt(target: Path, receipt: dict) -> None:
    target = output_target(target)
    target.parent.mkdir(parents=True, exist_ok=True)
    # O_EXCL/mkstemp avoids following an attacker-created temporary symlink.
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=target.parent,
                                     prefix=".publication-", delete=False) as stream:
        temporary = Path(stream.name)
        try:
            json.dump(receipt, stream, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
            output_target(target)
            os.replace(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)


def publish(*, source: Path, selection: Path, commit: str, pipeline_id: str,
            output: Path, scratch_root: Path) -> dict:
    token = os.environ.get("GITHUB_PUBLIC_TOKEN", "")
    if not token or any(character in token for character in "\r\n\0"):
        raise ReleaseError("GITHUB_PUBLIC_TOKEN is required and must be a single line")
    raw = publisher.read_regular_text(selection, maximum_bytes=publisher.MAX_CONTRACT_BYTES,
                                      label="public selection")
    publisher.validate_selection(publisher.parse_json_document(raw, label="public selection"),
                                 expected_commit=commit, expected_pipeline_id=pipeline_id)
    if publisher.git(source, "rev-parse", "--verify", f"{commit}^{{commit}}").decode().strip() != commit:
        raise ReleaseError("source commit differs from the selection")
    output_target(output)
    scratch_root = scratch_root.absolute()
    # Reject symlinked parents before creating the private authentication scope.
    output_target(scratch_root / ".boundary")
    scratch_root.mkdir(parents=True, exist_ok=True)
    client = GitHub(token)
    authorize(client)
    with tempfile.TemporaryDirectory(prefix="github-public-", dir=scratch_root) as directory:
        private = Path(directory)
        private.chmod(0o700)
        frozen_selection = private / "selection.json"
        frozen_selection.write_text(raw, encoding="utf-8")
        askpass = private / "askpass"
        askpass.write_text(
            '#!/bin/sh\ncase "$1" in\n'
            '  *Username*) printf "%s\\n" "x-access-token" ;;\n'
            '  *Password*) printf "%s\\n" "$GITHUB_PUBLIC_TOKEN" ;;\n'
            '  *) exit 1 ;;\nesac\n', encoding="utf-8")
        askpass.chmod(0o700)
        receipt = publisher.publish(
            source=source, release=frozen_selection, expected_commit=commit,
            expected_pipeline_id=pipeline_id, remote=REMOTE, scratch_root=private,
            authentication_environment={"GIT_ASKPASS": str(askpass), "GITHUB_PUBLIC_TOKEN": token})
        release, release_status = ensure_release(client, receipt)
    result = {"version": receipt["version"], "source": commit, "pipeline_id": pipeline_id,
              "public_commit": receipt["public_commit"], "status": receipt["status"],
              "release_status": release_status, "release_id": release["id"],
              "release_url": release["html_url"]}
    write_receipt(output, result)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "selection", "output", "scratch-root"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--pipeline-id", required=True)
    try:
        result = publish(**vars(parser.parse_args()))
    except ReleaseError as error:
        print(f"public release refused: {error}", file=sys.stderr)
        return 1
    except publisher.PublishError as error:
        # Snapshot validation messages are fixed strings and safe to report.
        print(f"public release refused: {error}", file=sys.stderr)
        return 1
    except (ValueError, OSError, UnicodeError) as error:
        # Never expose exception bodies, Git diagnostics, HTTP bodies or paths.
        print(f"public release refused: {type(error).__name__}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
