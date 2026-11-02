#!/usr/bin/env python3
"""Select a Catalog stable proposal and validate the internal release candidate."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import stat
import sys
from typing import Any

if __package__:
    from .apply_release_bundle import (
        AdapterError, GIT_COMMIT, MAX_CONTRACT_BYTES, PIPELINE_ID,
        read_regular_text, reject_duplicate_json_keys,
    )
else:
    from apply_release_bundle import (
        AdapterError, GIT_COMMIT, MAX_CONTRACT_BYTES, PIPELINE_ID,
        read_regular_text, reject_duplicate_json_keys,
    )

SELECTION_SCHEMA = "piping.public-selection/v1"
STABLE = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
)
SELECTION_FIELDS = {"schema", "commit", "pipeline_id", "pipeline_iid", "version"}
CATALOG_STRINGS = ("version", "core", "major", "minor", "patch", "prerelease", "build_metadata", "channel", "source", "release_line", "release_bump", "git_tag", "image_tag", "build_id", "git_sha")
CATALOG_BOOLEANS = ("is_release", "is_release_candidate", "is_prerelease")


def parse_json_document(raw: str, *, label: str) -> Any:
    try:
        return json.loads(raw, object_pairs_hook=reject_duplicate_json_keys)
    except (ValueError, RecursionError) as error:
        raise AdapterError(f"{label} is not canonical JSON") from error


def validate_identity(commit: str, pipeline_id: str, pipeline_iid: str) -> None:
    if not isinstance(commit, str) or GIT_COMMIT.fullmatch(commit) is None:
        raise AdapterError("selection commit is not a full lowercase Git object ID")
    if not isinstance(pipeline_id, str) or PIPELINE_ID.fullmatch(pipeline_id) is None:
        raise AdapterError("selection pipeline ID is not canonical")
    if not isinstance(pipeline_iid, str) or PIPELINE_ID.fullmatch(pipeline_iid) is None:
        raise AdapterError("selection pipeline IID is not canonical")


def validate_catalog(value: object, *, commit: str, pipeline_id: str,
                     pipeline_iid: str, internal: bool) -> str:
    if not isinstance(value, dict):
        raise AdapterError("Catalog version is not a JSON object")
    for name in CATALOG_STRINGS:
        if not isinstance(value.get(name), str):
            raise AdapterError(f"Catalog version field {name} must be a string")
    for name in CATALOG_BOOLEANS:
        if not isinstance(value.get(name), bool):
            raise AdapterError(f"Catalog version field {name} must be boolean")
    core = value["core"]
    match = STABLE.fullmatch(core)
    if match is None:
        raise AdapterError("Catalog core is not stable SemVer")
    major, minor, patch = match.groups()
    version = f"{core}-rc.{pipeline_id}" if internal else core
    expected = {
        "version": version, "major": major, "minor": minor, "patch": patch,
        "prerelease": f"rc.{pipeline_id}" if internal else "", "build_metadata": "",
        "channel": "rc" if internal else "stable", "source": "public-next",
        "release_line": f"{major}.{minor}", "git_tag": version, "image_tag": version,
        "build_id": pipeline_iid, "git_sha": commit, "is_release": False,
        "is_release_candidate": True, "is_prerelease": internal,
    }
    if any(value[key] != expected_value for key, expected_value in expected.items()):
        raise AdapterError("Catalog version identity or fields are inconsistent")
    if value["release_bump"] not in ({"", "patch"} if internal else {"patch", "minor", "major"}):
        raise AdapterError("Catalog release bump is invalid")
    return core


def validate_selection(document: object, *, expected_commit: str,
                       expected_pipeline_id: str) -> dict[str, str]:
    if not isinstance(document, dict) or set(document) != SELECTION_FIELDS:
        raise AdapterError("public selection has an unsupported top-level schema")
    commit, pipeline_id, pipeline_iid = (document[k] for k in ("commit", "pipeline_id", "pipeline_iid"))
    validate_identity(commit, pipeline_id, pipeline_iid)
    if document["schema"] != SELECTION_SCHEMA or commit != expected_commit or pipeline_id != expected_pipeline_id:
        raise AdapterError("public selection identity is inconsistent")
    if not isinstance(document["version"], str) or STABLE.fullmatch(document["version"]) is None:
        raise AdapterError("public selection version is not stable SemVer")
    return {k: document[k] for k in ("version", "commit", "pipeline_id", "pipeline_iid")}


def build_selection(proposal: object, internal: object, *, commit: str,
                    pipeline_id: str, pipeline_iid: str) -> dict[str, Any]:
    validate_identity(commit, pipeline_id, pipeline_iid)
    version = validate_catalog(proposal, commit=commit, pipeline_id=pipeline_id,
                               pipeline_iid=pipeline_iid, internal=False)
    validate_catalog(internal, commit=commit, pipeline_id=pipeline_id,
                     pipeline_iid=pipeline_iid, internal=True)
    return {"schema": SELECTION_SCHEMA, "commit": commit, "pipeline_id": pipeline_id,
            "pipeline_iid": pipeline_iid, "version": version}


def write_selection(target: Path, document: dict[str, Any]) -> None:
    try:
        target_stat = target.lstat()
    except FileNotFoundError:
        target_stat = None
    if target_stat is not None and (
        stat.S_ISLNK(target_stat.st_mode) or not stat.S_ISREG(target_stat.st_mode)
    ):
        raise AdapterError("selection output must be a regular non-symlink file")
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.with_name(f".{target.name}.tmp-{os.getpid()}")
    if temporary.exists() or temporary.is_symlink():
        raise AdapterError("temporary selection path already exists")
    try:
        temporary.write_text(
            json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        os.replace(temporary, target)
    except (OSError, UnicodeError) as error:
        try:
            temporary.unlink(missing_ok=True)
        except OSError:
            pass
        raise AdapterError("cannot write the public selection") from error


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--proposal", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--pipeline-id", required=True)
    parser.add_argument("--pipeline-iid", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        proposal = parse_json_document(read_regular_text(
            args.proposal, maximum_bytes=MAX_CONTRACT_BYTES, label="stable proposal"), label="stable proposal")
        internal = parse_json_document(read_regular_text(
            args.candidate, maximum_bytes=MAX_CONTRACT_BYTES, label="internal candidate"), label="internal candidate")
        document = build_selection(proposal, internal, commit=args.commit,
                                   pipeline_id=args.pipeline_id, pipeline_iid=args.pipeline_iid)
        write_selection(args.output, document)
    except (AdapterError, OSError, UnicodeError) as error:
        # Boundary validators have safe fixed diagnostics; filesystem errors may
        # include operator-controlled paths, so do not print those exceptions.
        detail = str(error) if isinstance(error, AdapterError) else "invalid local input"
        print(f"public selection refused: {detail}", file=sys.stderr)
        return 1
    print(json.dumps(
        {"output": str(args.output),
         "version": document["version"]}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
