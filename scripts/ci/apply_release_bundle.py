#!/usr/bin/env python3
"""Apply a validated release-bundle contract to a Kustomize image block."""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
import sys
import tempfile
from pathlib import Path
from typing import Any

MAX_CONTRACT_BYTES = 1024 * 1024
MAX_KUSTOMIZATION_BYTES = 1024 * 1024
DIGEST = re.compile(r"sha256:[0-9a-f]{64}")
GIT_COMMIT = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})")
PIPELINE_ID = re.compile(r"[1-9][0-9]*")
ARTIFACT_KEY = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")
REPOSITORY = re.compile(
    r"(?:(?:localhost|[a-z0-9.-]+\.[a-z0-9.-]+)(?::[0-9]+)?|"
    r"[a-z0-9.-]+:[0-9]+)/[a-z0-9._/-]+"
)
SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\."
    r"(0|[1-9][0-9]*)\."
    r"(0|[1-9][0-9]*)"
    r"(?:-((?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)

RELEASE_SCHEMA = "ci-components.release/v1"

TOP_LEVEL_FIELDS = {
    "schema",
    "release_id",
    "version",
    "commit",
    "pipeline_id",
    "images",
    "files",
}
IMAGE_FIELDS = {
    "artifact_key",
    "repository",
    "digest",
    "digest_ref",
    "platforms",
    "build_job",
}


class AdapterError(ValueError):
    """A release or Kustomize contract violated the deployment boundary."""


def reject_duplicate_json_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise AdapterError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def read_regular_text(path: Path, *, maximum_bytes: int, label: str) -> str:
    try:
        file_stat = path.lstat()
    except FileNotFoundError as error:
        raise AdapterError(f"{label} is missing") from error
    if stat.S_ISLNK(file_stat.st_mode) or not stat.S_ISREG(file_stat.st_mode):
        raise AdapterError(f"{label} must be a regular non-symlink file")
    if file_stat.st_size > maximum_bytes:
        raise AdapterError(f"{label} exceeds the size limit")
    try:
        return path.read_text(encoding="utf-8")
    except UnicodeDecodeError as error:
        raise AdapterError(f"{label} is not UTF-8") from error


def parse_expected_images(values: list[str]) -> dict[str, str]:
    expected: dict[str, str] = {}
    seen_repositories: set[str] = set()
    for value in values:
        artifact_key, separator, repository = value.partition("=")
        if separator != "=" or ARTIFACT_KEY.fullmatch(artifact_key) is None:
            raise AdapterError("--image must use ARTIFACT_KEY=LITERAL_REPOSITORY")
        if REPOSITORY.fullmatch(repository) is None:
            raise AdapterError(f"invalid repository for image {artifact_key}")
        if artifact_key in expected:
            raise AdapterError(f"duplicate expected image key: {artifact_key}")
        if repository in seen_repositories:
            raise AdapterError(f"duplicate expected repository: {repository}")
        expected[artifact_key] = repository
        seen_repositories.add(repository)
    if not expected:
        raise AdapterError("at least one --image contract is required")
    return expected


def load_release(
    release_path: Path,
    *,
    expected_commit: str,
    expected_pipeline_id: str,
    expected_platform: str,
    expected_images: dict[str, str],
) -> dict[str, str]:
    raw = read_regular_text(
        release_path,
        maximum_bytes=MAX_CONTRACT_BYTES,
        label="release contract",
    )
    try:
        release = json.loads(raw, object_pairs_hook=reject_duplicate_json_keys)
    except (json.JSONDecodeError, AdapterError) as error:
        raise AdapterError("release contract is not canonical JSON") from error
    if not isinstance(release, dict) or set(release) != TOP_LEVEL_FIELDS:
        raise AdapterError("release contract has an unsupported top-level schema")
    # Use the catalog's current schema, without older-schema aliases.
    if release["schema"] != RELEASE_SCHEMA:
        raise AdapterError("release contract schema is unsupported")
    if release["files"] != []:
        raise AdapterError("image deployment cannot consume file artifacts")
    if GIT_COMMIT.fullmatch(expected_commit) is None:
        raise AdapterError("expected commit is not a full lowercase Git object ID")
    if PIPELINE_ID.fullmatch(expected_pipeline_id) is None:
        raise AdapterError("expected pipeline ID is not canonical")
    if release["commit"] != expected_commit:
        raise AdapterError("release contract belongs to another commit")
    if str(release["pipeline_id"]) != expected_pipeline_id:
        raise AdapterError("release contract belongs to another pipeline")
    if (
        not isinstance(release["version"], str)
        or SEMVER.fullmatch(release["version"]) is None
    ):
        raise AdapterError("release version is not strict SemVer")
    expected_release_id = f"release-{expected_pipeline_id}-{expected_commit[:12]}"
    if release["release_id"] != expected_release_id:
        raise AdapterError("release ID is inconsistent with pipeline and commit")
    if not isinstance(release["images"], list):
        raise AdapterError("release images must be an array")

    digests: dict[str, str] = {}
    for image in release["images"]:
        if not isinstance(image, dict) or set(image) != IMAGE_FIELDS:
            raise AdapterError("release image has an unsupported schema")
        artifact_key = image["artifact_key"]
        if not isinstance(artifact_key, str) or artifact_key not in expected_images:
            raise AdapterError("release contains an unexpected image")
        if artifact_key in digests:
            raise AdapterError(f"release contains duplicate image: {artifact_key}")
        repository = expected_images[artifact_key]
        if image["repository"] != repository:
            raise AdapterError(f"repository mismatch for image {artifact_key}")
        digest = image["digest"]
        if not isinstance(digest, str) or DIGEST.fullmatch(digest) is None:
            raise AdapterError(f"invalid digest for image {artifact_key}")
        if image["digest_ref"] != f"{repository}@{digest}":
            raise AdapterError(f"digest reference mismatch for image {artifact_key}")
        if image["platforms"] != expected_platform:
            raise AdapterError(f"platform mismatch for image {artifact_key}")
        if image["build_job"] != f"image:{artifact_key}:build":
            raise AdapterError(f"build identity mismatch for image {artifact_key}")
        digests[artifact_key] = digest

    if set(digests) != set(expected_images):
        raise AdapterError("release image set is incomplete")
    return digests


def render_kustomization(
    source: str,
    *,
    expected_images: dict[str, str],
    digests: dict[str, str],
) -> str:
    lines = source.splitlines(keepends=True)
    image_block_starts = [
        index
        for index, line in enumerate(lines)
        if re.fullmatch(r"images:\s*(?:#.*)?(?:\r?\n)?", line)
    ]
    if len(image_block_starts) != 1:
        raise AdapterError(
            "kustomization must contain exactly one top-level images block"
        )
    block_start = image_block_starts[0] + 1
    block_end = len(lines)
    for index in range(block_start, len(lines)):
        line_without_newline = lines[index].rstrip("\r\n")
        if line_without_newline and not line_without_newline.startswith(
            (" ", "\t", "#")
        ):
            block_end = index
            break

    entry_pattern = re.compile(
        r"^  -\s+name:\s*[\"']?([^\"'\s#]+)[\"']?\s*(?:#.*)?(?:\r?\n)?$"
    )
    entries: dict[str, list[int]] = {}
    entry_starts: list[tuple[int, str]] = []
    for index in range(block_start, block_end):
        match = entry_pattern.fullmatch(lines[index])
        if match is None:
            continue
        repository = match.group(1)
        entries.setdefault(repository, []).append(index)
        entry_starts.append((index, repository))

    artifact_by_selector = {
        f"piping-{artifact_key}": artifact_key for artifact_key in expected_images
    }
    for selector in artifact_by_selector:
        if len(entries.get(selector, [])) != 1:
            raise AdapterError(
                f"kustomization must contain image selector exactly once: {selector}"
            )

    entry_starts.sort()
    entry_end_by_start: dict[int, int] = {}
    for position, (entry_start, _) in enumerate(entry_starts):
        next_start = (
            entry_starts[position + 1][0]
            if position + 1 < len(entry_starts)
            else block_end
        )
        entry_end_by_start[entry_start] = next_start

    field_pattern = re.compile(
        r"^(?P<indent>\s+)(?P<field>newName|newTag|digest):.*?(?P<ending>\r?\n)?$"
    )
    for selector, indexes in entries.items():
        if selector not in artifact_by_selector:
            continue
        entry_start = indexes[0]
        matching_fields: list[tuple[int, re.Match[str]]] = []
        repository_fields: list[tuple[int, re.Match[str]]] = []
        for index in range(entry_start + 1, entry_end_by_start[entry_start]):
            match = field_pattern.fullmatch(lines[index])
            if match is not None:
                if match.group("field") == "newName":
                    repository_fields.append((index, match))
                else:
                    matching_fields.append((index, match))
        if len(matching_fields) != 1:
            raise AdapterError(
                f"kustomization image must contain exactly one newTag or digest: {selector}"
            )
        if len(repository_fields) != 1:
            raise AdapterError(
                f"kustomization image must contain exactly one newName: {selector}"
            )
        repository_index, repository_match = repository_fields[0]
        current_repository = lines[repository_index].split(":", 1)[1].strip()
        current_repository = current_repository.split(" #", 1)[0].strip().strip("\"'")
        if REPOSITORY.fullmatch(current_repository) is None:
            raise AdapterError(f"kustomization newName must be a literal repository: {selector}")
        artifact_key = artifact_by_selector[selector]
        lines[repository_index] = (
            f'{repository_match.group("indent")}newName: "{expected_images[artifact_key]}"'
            f'{repository_match.group("ending") or ""}'
        )
        field_index, field_match = matching_fields[0]
        ending = field_match.group("ending") or ""
        lines[field_index] = (
            f'{field_match.group("indent")}digest: "{digests[artifact_key]}"{ending}'
        )
    return "".join(lines)


def atomic_write(path: Path, content: str) -> None:
    original_mode = stat.S_IMODE(path.stat().st_mode)
    temporary_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=path.parent,
            prefix=f".{path.name}.",
            delete=False,
        ) as temporary_file:
            temporary_name = temporary_file.name
            temporary_file.write(content)
            temporary_file.flush()
            os.fsync(temporary_file.fileno())
        os.chmod(temporary_name, original_mode)
        os.replace(temporary_name, path)
        temporary_name = None
    finally:
        if temporary_name is not None:
            Path(temporary_name).unlink(missing_ok=True)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gitops-dir", type=Path, default=None)
    parser.add_argument("--release", required=True, type=Path)
    parser.add_argument("--kustomization", required=True, type=Path)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--pipeline-id", required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument(
        "--image", action="append", default=[], metavar="KEY=REPOSITORY"
    )
    return parser.parse_args()


def run() -> None:
    args = parse_args()
    if not args.kustomization.is_absolute():
        if args.gitops_dir is None:
            raise SystemExit(
                "--kustomization must be absolute or --gitops-dir must be set"
            )
        args.kustomization = args.gitops_dir / args.kustomization
    expected_images = parse_expected_images(args.image)
    digests = load_release(
        args.release,
        expected_commit=args.commit,
        expected_pipeline_id=args.pipeline_id,
        expected_platform=args.platform,
        expected_images=expected_images,
    )
    source = read_regular_text(
        args.kustomization,
        maximum_bytes=MAX_KUSTOMIZATION_BYTES,
        label="kustomization",
    )
    rendered = render_kustomization(
        source,
        expected_images=expected_images,
        digests=digests,
    )
    if rendered != source:
        atomic_write(args.kustomization, rendered)
    print(
        f"Applied {len(digests)} immutable image digests from the validated release contract."
    )


def main() -> int:
    try:
        run()
    except (AdapterError, OSError) as error:
        print(f"deploy adapter: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
