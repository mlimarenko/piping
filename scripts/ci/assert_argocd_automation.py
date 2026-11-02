#!/usr/bin/env python3
"""Refuse a deploy when Argo CD would not apply it.

The deploy job's whole effect is a commit pushed to the manifests repository.
Whether that commit ever reaches a cluster is Argo CD's business, and it only
does so while the Application keeps `syncPolicy.automated` with `selfHeal`.
Turn automation off — by hand during an incident, by a merge that drops the
block, by `automated: null` that an operator meant to be temporary — and the
push still succeeds, the pipeline still goes green, and the release sits in
Git untouched. That failure has already cost this project three days of a
broken deploy nobody could see.

So the assertion runs before the push, against the manifests actually cloned,
and refuses rather than reporting a success that did not happen.

Deliberately NOT checked here: whether the cluster is reachable or the new
pods are healthy. Neither is knowable from this runner — the regional hosts do
not resolve under split-horizon DNS — and a check that cannot see its subject
is worse than an honest gap.

The Application is found by searching the region's directory for the manifest
whose `metadata.name` matches, rather than by a literal path. A literal path
is what turned this guard into the failure it exists to prevent: the manifests
repository reorganised its apps into subdirectories, the path went stale, and
the first pipeline to reach the deploy stage refused with "file is missing"
about an Application that was there all along.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import yaml

# A file large enough to be something other than an Application manifest is
# not worth parsing; refusing beats loading whatever it is.
MAX_MANIFEST_BYTES = 256 * 1024


def refuse(message: str) -> None:
    print(f"Deploy refused: {message}", file=sys.stderr)
    raise SystemExit(1)


def find_application(root: Path, name: str) -> Path:
    """The one manifest under `root` declaring an Application called `name`."""
    if not root.is_dir():
        refuse(f"{root} is not a directory — the region's manifests are not where expected.")
    matches = []
    for path in sorted(root.rglob("*.yaml")):
        if path.stat().st_size > MAX_MANIFEST_BYTES:
            continue
        try:
            documents = list(yaml.safe_load_all(path.read_text(encoding="utf-8")))
        except yaml.YAMLError:
            continue
        for document in documents:
            if (
                isinstance(document, dict)
                and document.get("kind") == "Application"
                and (document.get("metadata") or {}).get("name") == name
            ):
                matches.append(path)
                break
    if not matches:
        refuse(
            f"no Argo CD Application named {name!r} under {root} — either it was removed "
            "or this deploy is pointed at the wrong region."
        )
    if len(matches) > 1:
        refuse(
            f"{len(matches)} Applications named {name!r} under {root}: "
            + ", ".join(str(match) for match in matches)
        )
    return matches[0]


def automation_of(path: Path) -> dict[str, object]:
    if not path.is_file():
        refuse(f"{path} is missing — the Application it describes cannot be checked.")
    size = path.stat().st_size
    if size > MAX_MANIFEST_BYTES:
        refuse(f"{path} is {size} bytes, larger than an Application manifest should be.")

    documents = [
        document
        for document in yaml.safe_load_all(path.read_text(encoding="utf-8"))
        if isinstance(document, dict)
    ]
    applications = [
        document for document in documents if document.get("kind") == "Application"
    ]
    if len(applications) != 1:
        refuse(
            f"{path} holds {len(applications)} Argo CD Applications; expected exactly one, "
            "so there is no doubt about which one governs this deploy."
        )

    policy = applications[0].get("spec", {}).get("syncPolicy") or {}
    automated = policy.get("automated")
    if not isinstance(automated, dict):
        refuse(
            f"{path} has no syncPolicy.automated — Argo CD would never apply the release "
            "this job is about to push."
        )
    return automated


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--gitops-dir",
        type=Path,
        default=None,
        help="root of the cloned GitOps repository; resolves relative --region-root",
    )
    parser.add_argument(
        "--region-root",
        action="append",
        required=True,
        type=Path,
        help="directory holding one region's Argo CD manifests; repeat per region",
    )
    parser.add_argument(
        "--application-name",
        required=True,
        help="metadata.name of the Application this deploy feeds",
    )
    args = parser.parse_args()

    for root in args.region_root:
        if not root.is_absolute():
            if args.gitops_dir is None:
                parser.error("--region-root must be absolute or --gitops-dir must be set")
            root = args.gitops_dir / root
        path = find_application(root, args.application_name)
        automated = automation_of(path)
        # `selfHeal` is the half that matters for a GitOps push: without it
        # Argo CD applies a change only when it happens to notice, and a
        # cluster that has drifted stays drifted.
        if automated.get("selfHeal") is not True:
            refuse(
                f"{path} has automated sync without selfHeal; a pushed release would not "
                "be reconciled onto a cluster that has drifted."
            )
        print(f"  {path}: automated sync with self-healing — release will be applied.")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
