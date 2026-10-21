#!/usr/bin/env python3
"""Select one natively reserved version as the canonical public release input."""

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
SELECTION_FIELDS = {"schema", "commit", "pipeline_id", "pipeline_iid", "reservation"}
# The tag job always writes these fields; additional documented fields are
# preserved verbatim because the native owner, not this selector, defines them.
RESERVED_STRING_FIELDS = (
    "version", "core", "major", "minor", "patch", "prerelease",
    "build_metadata", "channel", "source", "release_line", "release_bump",
    "git_tag", "image_tag", "build_id", "git_sha",
)
RESERVED_BOOLEAN_FIELDS = (
    "is_release", "is_release_candidate", "is_prerelease", "tag_reserved",
)


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


def validate_reservation(
    reservation: object, *, commit: str, pipeline_iid: str
) -> dict[str, str]:
    """Return the stable public version selected by a native reserved artifact."""
    if not isinstance(reservation, dict):
        raise AdapterError("reserved version is not a JSON object")
    for name in RESERVED_STRING_FIELDS:
        if not isinstance(reservation.get(name), str):
            raise AdapterError(f"reserved version field {name} must be a string")
    for name in RESERVED_BOOLEAN_FIELDS:
        if not isinstance(reservation.get(name), bool):
            raise AdapterError(f"reserved version field {name} must be boolean")

    version = reservation["version"]
    match = STABLE.fullmatch(version)
    if match is None:
        raise AdapterError("reserved version is not a stable X.Y.Z SemVer")
    major, minor, patch = match.groups()
    if reservation["core"] != version:
        raise AdapterError("reserved version core is inconsistent")
    if (reservation["major"], reservation["minor"], reservation["patch"]) != (
        major, minor, patch
    ):
        raise AdapterError("reserved version components are inconsistent")
    if reservation["source"] not in {"release", "pipeline"}:
        raise AdapterError("reserved version source is invalid")
    if reservation["prerelease"] or reservation["build_metadata"]:
        raise AdapterError("reserved version is not stable")
    if reservation["channel"] != "stable":
        raise AdapterError("reserved version channel is not stable")
    if reservation["release_line"] != f"{major}.{minor}":
        raise AdapterError("reserved version release line is inconsistent")
    if reservation["image_tag"] != version:
        raise AdapterError("reserved version image tag is inconsistent")
    if reservation["git_tag"] != version:
        raise AdapterError("reserved version Git tag is inconsistent")
    if reservation["git_sha"] != commit:
        raise AdapterError("reserved version belongs to another commit")
    if reservation["build_id"] != pipeline_iid:
        raise AdapterError("reserved version belongs to another pipeline IID")
    if reservation["tag_reserved"] is not True:
        raise AdapterError("reserved version tag is not reserved")
    if reservation["is_release_candidate"] is not True:
        raise AdapterError("reserved version is not a release candidate")
    if reservation["is_prerelease"] is not False:
        raise AdapterError("reserved version is a prerelease")
    return {"version": version, "commit": commit, "pipeline_iid": pipeline_iid}


def validate_selection(
    document: object, *, expected_commit: str, expected_pipeline_id: str
) -> dict[str, str]:
    """Validate the exact public-selection wrapper consumed by the publisher."""
    if not isinstance(expected_commit, str) or GIT_COMMIT.fullmatch(expected_commit) is None:
        raise AdapterError("expected commit is not a full lowercase Git object ID")
    if (
        not isinstance(expected_pipeline_id, str)
        or PIPELINE_ID.fullmatch(expected_pipeline_id) is None
    ):
        raise AdapterError("expected pipeline ID is not canonical")
    if not isinstance(document, dict) or set(document) != SELECTION_FIELDS:
        raise AdapterError("public selection has an unsupported top-level schema")
    if document["schema"] != SELECTION_SCHEMA:
        raise AdapterError("public selection schema is unsupported")
    commit = document["commit"]
    pipeline_id = document["pipeline_id"]
    pipeline_iid = document["pipeline_iid"]
    validate_identity(commit, pipeline_id, pipeline_iid)
    if commit != expected_commit:
        raise AdapterError("public selection belongs to another commit")
    if pipeline_id != expected_pipeline_id:
        raise AdapterError("public selection belongs to another pipeline")
    selected = validate_reservation(
        document["reservation"], commit=commit, pipeline_iid=pipeline_iid
    )
    selected["pipeline_id"] = pipeline_id
    return selected


def build_selection(
    reservation: object, *, commit: str, pipeline_id: str, pipeline_iid: str
) -> dict[str, Any]:
    validate_identity(commit, pipeline_id, pipeline_iid)
    validate_reservation(reservation, commit=commit, pipeline_iid=pipeline_iid)
    return {
        "schema": SELECTION_SCHEMA,
        "commit": commit,
        "pipeline_id": pipeline_id,
        "pipeline_iid": pipeline_iid,
        "reservation": reservation,
    }


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
    parser.add_argument("--reservation", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--pipeline-id", required=True)
    parser.add_argument("--pipeline-iid", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        raw = read_regular_text(
            args.reservation, maximum_bytes=MAX_CONTRACT_BYTES,
            label="reserved version",
        )
        reservation = parse_json_document(raw, label="reserved version")
        document = build_selection(
            reservation, commit=args.commit, pipeline_id=args.pipeline_id,
            pipeline_iid=args.pipeline_iid,
        )
        write_selection(args.output, document)
    except (AdapterError, OSError, UnicodeError) as error:
        # Boundary validators have safe fixed diagnostics; filesystem errors may
        # include operator-controlled paths, so do not print those exceptions.
        detail = str(error) if isinstance(error, AdapterError) else "invalid local input"
        print(f"public selection refused: {detail}", file=sys.stderr)
        return 1
    print(json.dumps(
        {"output": str(args.output),
         "version": document["reservation"]["version"]}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
