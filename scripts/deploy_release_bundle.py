#!/usr/bin/env python3
"""Guarded Compose updates from published bundles or locally selected images."""

from __future__ import annotations

import argparse
import base64
import contextlib
import fcntl
import hashlib
import io
import ipaddress
import json
import os
import re
import signal
import selectors
import time
import traceback
import shutil
import socket
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import uuid
from urllib.parse import unquote, urlsplit
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any, Never

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scripts.ci import postgres_migration_contract as migration_contract
from scripts.ci.apply_release_bundle import (
    load_release,
    parse_expected_images,
    reject_duplicate_json_keys,
)
from scripts.ci.postgres_migration_contract import (
    BASELINE_SQLX_SHA384, CATALOG_SQL, CHECK_BINDINGS_SQL, catalog_objects, canonical_readiness,
    definition_hash, exact_target_preservation, historical_check_forms,
    preservation_extensions, prove_native_check, prove_original_native_checks, forward_catalog_delta,
    SERVICE_MAPPING_TABLE, SERVICE_TABLES, SERVICE_CUTOVER_VALIDATE_SQL,
    service_cutover_pending, service_cutover_expression,
    pending_transforms, transformation_columns, FHRP_TABLE, FHRP_CUTOVER_VALIDATE_SQL,
    CONTENT_TYPE_SEED_MIGRATIONS, CONTENT_TYPE_TABLE, content_type_seeds_pending,
    TAG_CUTOVER_PATH, TAG_CUTOVER_SHA384, TAG_SELECTOR_TABLE, TAG_CATALOG_TABLE,
    TAG_FACTORY_ROW, tag_cutover_fields,
    DATAFLOW_CUTOVER_PATH, DATAFLOW_CUTOVER_SHA384, DATAFLOW_CHANGED_TABLES,
    DATAFLOW_CONTENT_TYPE_SEEDS, DATAFLOW_FLOW_TABLE, DATAFLOW_NEW_TABLES,
    DATAFLOW_REMOVED_TABLE, DATAFLOW_SELECTOR_IP_TABLE, DATAFLOW_SELECTOR_TABLE,
    dataflow_cutover_expression,
    workspace_0016_delta, workspace_0016_part, workspace_0016_conservation, WORKSPACE_0016_RETIRED_TABLES,
    validate_baseline_layout_contract, baseline_layout_positions,
)
from scripts.ci.compose_delivery_contract import (
    canonical, package_binding, preservation_manifest, save_authority, save_identity, verify_manifest,
    manual_old_availability_result, compose_abort_acknowledgment,
)

ROLES = {
    "backend": "PIPING_BACKEND_IMAGE",
    "frontend": "PIPING_FRONTEND_IMAGE",
    "ansible-runner": "PIPING_ANSIBLE_RUNNER_IMAGE",
}
COMPOSE_FILE_DEFAULT = "compose.yaml"
ENV_FILE = ".env"
RUNNER_HEALTH_COMMAND = ["CMD", "/usr/local/bin/piping-ansible-runner", "--health-check"]
# The shipped predecessor's unauthenticated check must be replaced, not retained.
RETIRED_RUNNER_HEALTH_COMMAND = [
    "CMD-SHELL", "python3 -c \"import urllib.request; urllib.request.urlopen('http://localhost:8080/health')\" || exit 1",
]
JOB_NETWORK_ACTIVATION = "piping.job-network-activation/v1"
JOB_NETWORK_DIRECTORY = "job-network"
JOB_NETWORK_OVERLAY = "guarded.compose.yml"
JOB_NETWORK_ACTIVATION_FILE = "activation.json"
JOB_NETWORK_IDENTITY_FILE = "network.identity.json"
JOB_NETWORK_RUNNER_UNIT_FILE = "runner-unit"
JOB_NETWORK_DEACTIVATION_FILE = "deactivated.json"
WRITERS = ("backend", "ansible-runner", "proxy")
PHASES = (
    "staging",
    "staged",
    "backup-starting",
    "writers-stopping",
    "writers-stopped",
    "dumping",
    "backup-verified",
    "restore-starting",
    "restore-verified",
    "apply-starting",
    "images-installing",
    "migrating",
    "migration-verified",
    "starting",
    "started-awaiting-health",
    "healthy",
)
PREPARATION_PHASES = ("access-fenced", "original-r-dumping", "original-r-awaiting-proof", "original-r-verified", "converting-fhrp",
                      "prepared-c-verified", "recovering-original-r", "original-r-restored",
                      "starting-original-r", "original-r-started", "cutover-r-dumping", "cutover-r-awaiting-proof",
                      "cutover-r-verified", "recovering-backup", "backup-restored", "starting-backup", "backup-started",
                      "recovering-cutover-r", "cutover-r-restored", "starting-cutover-r", "cutover-r-started")
ABORTED_OLD_AVAILABILITY = "aborted-old-availability-restored"
PHASE_EDGES = {before: {after} for before, after in zip(PHASES, PHASES[1:])}
PHASE_EDGES["writers-stopped"] = {"access-fenced"}
PHASE_EDGES.update({"access-fenced": {"original-r-dumping", "cutover-r-dumping", "dumping"},
                   "original-r-dumping": {"original-r-awaiting-proof"},
                   "original-r-awaiting-proof": {"original-r-verified"},
                   "original-r-verified": {"converting-fhrp"},
                   "converting-fhrp": {"prepared-c-verified"}, "prepared-c-verified": {"dumping"},
                   "cutover-r-dumping": {"cutover-r-awaiting-proof"},
                   "cutover-r-awaiting-proof": {"cutover-r-verified"}, "cutover-r-verified": {"dumping"}})
for _phase in (*PHASES[PHASES.index("writers-stopped"):], *PREPARATION_PHASES[:8]):
    PHASE_EDGES.setdefault(_phase, set()).add("recovering-original-r")
PHASE_EDGES["recovering-original-r"] = {"original-r-restored"}
PHASE_EDGES["original-r-restored"] = {"starting-original-r"}
PHASE_EDGES["starting-original-r"] = {"original-r-started"}
for _source in ("backup", "cutover-r"):
    for _phase in (*PHASES[PHASES.index("backup-verified"):], "cutover-r-awaiting-proof", "cutover-r-verified"):
        PHASE_EDGES.setdefault(_phase, set()).add("recovering-" + _source)
    PHASE_EDGES["recovering-" + _source] = {_source + "-restored"}
    PHASE_EDGES[_source + "-restored"] = {"starting-" + _source}
    PHASE_EDGES["starting-" + _source] = {_source + "-started"}
PHASE_EDGES["backup-verified"].add(ABORTED_OLD_AVAILABILITY)
PHASE_EDGES["dumping"].add("recovering-cutover-r")
MAX_BYTES = 16 * 1024 * 1024
PROOF_POSTGRES_READY_SECONDS = 270
PROOF_POSTGRES_HEALTH_RETRIES = 60
POSTGRES_RESTORE_TIMEOUT_SECONDS = 900
COMMAND_DIAGNOSTICS = None


def helper_identity():
    paths = (Path(__file__), Path(__file__).parent / "ci/compose_delivery_contract.py",
             Path(__file__).parent / "ci/apply_release_bundle.py",
             Path(__file__).parent / "ci/postgres_migration_contract.py")
    return sha(canonical({p.name: file_sha(p.resolve()) for p in paths}))


def sign(path, key, signature):
    checked_path(key)
    require(stat.S_IMODE(key.stat().st_mode) & 0o077 == 0, "signing key must be private")
    write(signature, run(["openssl", "dgst", "-sha256", "-sign", str(key), str(path)]))


def authenticate(path, signature, key):
    for item in (path, signature, key):
        checked_path(item)
    run(["openssl", "dgst", "-sha256", "-verify", str(key), "-signature", str(signature), str(path)])


def save_image(image, directory, name, config, platform, revision=None):
    path = directory / (name + ".tar")
    checked_path(path, missing=True)
    with path.open("xb") as stream:
        os.chmod(path, 0o600)
        run(["docker", "image", "save", image], output=stream, timeout=300)
        stream.flush()
        os.fsync(stream.fileno())
    if config is None:
        with tarfile.open(path, mode="r:") as archive:
            manifest = json.loads(archive.extractfile("manifest.json").read(), object_pairs_hook=reject_duplicate_json_keys)
            require(isinstance(manifest, list) and len(manifest) == 1, "ambiguous installed native image")
            with archive.extractfile(manifest[0]["Config"]) as stream:
                config = "sha256:" + hashlib.file_digest(stream, "sha256").hexdigest()
    save_identity(path, config, platform, revision)
    return {"archive": path.name, "archive_sha256": file_sha(path), "size": path.stat().st_size,
            "config_digest": config}


def load_image(directory, description, platform, revision=None):
    name = description["archive"]
    require(isinstance(name, str) and Path(name).name == name, "unsafe archive name")
    path = checked_path(directory / name)
    require(path.stat().st_size == description["size"] and file_sha(path) == description["archive_sha256"],
            "image archive bytes changed")
    save_identity(path, description["config_digest"], platform, revision)
    loaded = run(["docker", "image", "load", "--input", str(path)], timeout=300).decode()
    # Docker save/load may intentionally discard RepoDigests. Config addressing is
    # native, not a tag guess; export verifies the actual content after loading.
    config = description["config_digest"]
    ids = re.findall(r"^Loaded image ID: (sha256:[0-9a-f]{64})$", loaded, re.MULTILINE)
    # Classic stores report a loaded tag when save includes RepoTags. Config is
    # the classic immutable ID; containerd reports its manifest ID explicitly.
    require(len(set(ids)) <= 1, "ambiguous native load result")
    local = ids[0] if ids else config
    item = json.loads(run(["docker", "image", "inspect", local]))
    require(len(item) == 1 and item[0]["Id"] == local, "loaded immutable image identity mismatch")
    exported_config(local, config, directory)
    return local


def verify_package(directory, key):
    checked_path(directory, directory=True)
    authenticate(directory / "package.json", directory / "package.sig", key)
    package = read_json(directory / "package.json")
    require(package["schema"] == "piping.compose-delivery/v1" and package["helper_sha256"] == helper_identity()
            and package["platform"] == "linux/amd64" and set(package["images"]) == set(ROLES),
            "package helper/platform/roles mismatch")
    require(package["source"] in ("published", "local"), "unsupported image selection authority")
    require(re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", package["commit"]), "backend source revision is missing")
    if package["source"] == "published":
        expected = {role: image["published_ref"].rsplit("@", 1)[0] for role, image in package["images"].items()}
        digests = load_release(directory / "release.json", expected_commit=package["commit"],
                               expected_pipeline_id=package["pipeline_id"], expected_platform=package["platform"],
                               expected_images=expected)
    else:
        selection = read_json(directory / "release.json")
        require(package["pipeline_id"] is None and set(selection) == {"schema", "images"}
                and selection["schema"] == "piping.compose-images/v1"
                and set(selection["images"]) == set(ROLES), "invalid local image selection")
        for role, image in package["images"].items():
            require(selection["images"][role] == {
                "image_id": image["selected_id"], "config_digest": image["config_digest"],
                "revision": image["revision"]}, "local image selection changed")
        require(package["images"]["backend"]["revision"] == package["commit"], "backend source differs from selection")
    require(file_sha(directory / "release.json") == package["release_sha256"], "release bytes changed")
    if "preservation" in package:
        description = package["preservation"]
        require(set(description) == {"manifest_sha256", "capture_sha256"}, "invalid preservation package attachment")
        require(file_sha(directory / "preservation.json") == description["manifest_sha256"]
                and (description["capture_sha256"] is None
                     or file_sha(directory / "check-capture.json") == description["capture_sha256"]),
                "preservation attachment bytes changed")
    loaded = {}
    for role, image in package["images"].items():
        require(Path(image["archive"]).name == image["archive"], "unsafe archive name")
        revision = image["revision"]
        require(revision is None or re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", revision), "invalid image source revision")
        if package["source"] == "published":
            require(revision == package["commit"]
                    and image["published_ref"] == expected[role] + "@" + digests[role], "release authority mismatch")
            verify_manifest(read(directory / (role + ".manifest.json")), image["published_ref"], image["config_digest"])
        else:
            save_authority(directory / image["archive"], image["selected_id"], image["config_digest"])
        identity = (image["archive_sha256"], image["config_digest"])
        if identity not in loaded:
            loaded[identity] = load_image(directory, image, package["platform"], revision)
        else:
            require(Path(image["archive"]).name == image["archive"], "unsafe archive name")
            path = checked_path(directory / image["archive"])
            require(path.stat().st_size == image["size"] and file_sha(path) == image["archive_sha256"],
                    "image archive bytes changed")
        image["local_id"] = loaded[identity]
    return package


def prepare(args):
    require(args.platform == "linux/amd64", "unsupported release platform")
    checked_path(args.package, missing=True)
    expected = parse_expected_images(args.image)
    require(set(expected) == set(ROLES), "three published image roles required")
    digests = load_release(args.release, expected_commit=args.commit, expected_pipeline_id=args.pipeline_id,
                           expected_platform=args.platform, expected_images=expected)
    args.package.mkdir(mode=0o700)
    write(args.package / "release.json", read(args.release))
    package = {"schema": "piping.compose-delivery/v1", "source": "published", "helper_sha256": helper_identity(),
               "commit": args.commit, "pipeline_id": args.pipeline_id, "platform": args.platform,
               "release_sha256": file_sha(args.package / "release.json"), "images": {}}
    for role in ROLES:
        ref = expected[role] + "@" + digests[role]
        raw = run(["docker", "buildx", "imagetools", "inspect", "--raw", ref], maximum=1024 * 1024)
        manifest = json.loads(raw, object_pairs_hook=reject_duplicate_json_keys)
        config = manifest["config"]["digest"]
        verify_manifest(raw, ref, config)
        write(args.package / (role + ".manifest.json"), raw)
        run(["docker", "pull", "--platform", args.platform, ref], timeout=300)
        inspected = json.loads(run(["docker", "image", "inspect", ref]))
        require(len(inspected) == 1 and ref in inspected[0].get("RepoDigests", []), "published pull identity mismatch")
        package["images"][role] = {"published_ref": ref, "revision": args.commit,
            **save_image(ref, args.package, role, config, args.platform, args.commit)}
    attach_preservation(args, package)
    write_json(args.package / "package.json", package)
    sign(args.package / "package.json", args.signing_key, args.package / "package.sig")


def attach_preservation(args, package):
    """Attach exact inventory authority, with capture only for historical exceptions."""
    if getattr(args, "preservation_manifest", None):
        capture_path = getattr(args, "check_capture", None)
        for private in (args.preservation_manifest, *([capture_path] if capture_path else [])):
            checked_path(private)
            require(stat.S_IMODE(private.stat().st_mode) == 0o600, "preservation inputs must be private")
        approval = read_json(args.preservation_manifest)
        preservation_manifest(approval, context=approval["context"], package_sha256=package_binding(package))
        require(approval["context"]["helper_sha256"] == package["helper_sha256"]
                and approval["context"]["application_source"] == package["commit"]
                and approval["context"]["release_sha256"] == package["release_sha256"]
                and approval["check_capture_sha256"] == (file_sha(capture_path) if capture_path else None),
                "preservation source/capture mismatch")
        write(args.package / "preservation.json", read(args.preservation_manifest))
        if capture_path:
            write(args.package / "check-capture.json", read(capture_path))
        package["preservation"] = {"manifest_sha256": file_sha(args.package / "preservation.json"),
                                    "capture_sha256": approval["check_capture_sha256"]}
    else:
        require(not getattr(args, "check_capture", None), "native CHECK capture requires explicit approval")


def prepare_local_package(directory, signing_key, images):
    """Seal explicit local image choices without claiming CI publication."""
    directory.mkdir(mode=0o700)
    package = {"schema": "piping.compose-delivery/v1", "source": "local",
               "helper_sha256": helper_identity(), "pipeline_id": None,
               "platform": "linux/amd64", "images": {}}
    selection = {"schema": "piping.compose-images/v1", "images": {}}
    for role, reference in images.items():
        inspected = json.loads(run(["docker", "image", "inspect", reference]))
        require(len(inspected) == 1, "ambiguous selected image")
        image = inspected[0]
        require(image["Os"] == "linux" and image["Architecture"] == "amd64", "unsupported selected image platform")
        revision = (image["Config"].get("Labels") or {}).get("org.opencontainers.image.revision")
        require(revision is None or re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", revision), "invalid selected image source")
        if role == "backend":
            require(revision is not None, "backend image must declare its source revision")
            package["commit"] = revision
        description = save_image(image["Id"], directory, role, None, package["platform"], revision)
        save_authority(directory / description["archive"], image["Id"], description["config_digest"])
        package["images"][role] = {"selected_id": image["Id"], "revision": revision, **description}
        selection["images"][role] = {"image_id": image["Id"], "revision": revision,
                                      "config_digest": description["config_digest"]}
    write_json(directory / "release.json", selection)
    package["release_sha256"] = file_sha(directory / "release.json")
    write_json(directory / "package.json", package)
    sign(directory / "package.json", signing_key, directory / "package.sig")


def local_update(args):
    """Run the existing guarded transaction locally with operator-selected images."""
    selected = {}
    for value in args.image:
        role, separator, reference = value.partition("=")
        require(separator == "=" and role in ROLES and role not in selected
                and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/@-]*", reference)
                and "://" not in reference, "invalid local image selection")
        selected[role] = reference
    require(selected, "select at least one image to update")
    args.stage = args.root / "backups" / datetime.now(timezone.utc).strftime("release-%Y%m%dT%H%M%SZ")
    deployment = Deployment(args)
    args.image_file = deployment.installed_image_file()
    global COMMAND_DIAGNOSTICS
    with deployment.lock():
        deployment.guard_recovery()
        deployment.plan = {"image_file": args.image_file}
        # Omitted roles retain the running immutable image, not a mutable tag.
        images = {role: deployment.service_image(role) for role in ROLES}
        services = deployment.running_declared_services()
        for role, reference in selected.items():
            if not args.skip_pull and not re.fullmatch(r"sha256:[0-9a-f]{64}", reference):
                run(["docker", "pull", "--platform", "linux/amd64", reference], timeout=300)
            images[role] = reference
        backups = args.root / "backups"
        if not backups.exists():
            backups.mkdir(mode=0o700)
        checked_path(backups, directory=True)
        require(stat.S_IMODE(backups.stat().st_mode) == 0o700, "backup directory must be private")
        operation = Path(tempfile.mkdtemp(prefix="operation-", dir=backups))
        COMMAND_DIAGNOSTICS = operation
        signing_key, public_key = operation / "operation.key", operation / "operation.pub"
        # This key binds one local operation. It is not publisher authentication.
        run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048",
             "-out", str(signing_key)])
        os.chmod(signing_key, 0o600)
        write(public_key, run(["openssl", "pkey", "-in", str(signing_key), "-pubout"]))
        args.package, args.verification_key = operation / "package", public_key
        args.disabled_schedule = None
        completed = False
        try:
            prepare_local_package(args.package, signing_key, images)
            deployment.stage_release()
            COMMAND_DIAGNOSTICS = deployment.stage
            deployment.backup()
            dossier = operation / "dossier"
            dossier.mkdir(mode=0o700)
            request = read_json(deployment.stage / "backup-request.json")
            files = {"plan.json", "backup-request.json", *request["files"], request["postgres_archive"]["archive"]}
            if request["installed_backend_archive"]:
                files.add(request["installed_backend_archive"]["archive"])
            for name in files:
                require(Path(name).name == name, "unsafe dossier member")
                source = checked_path(deployment.stage / name)
                destination = dossier / name
                checked_path(destination, missing=True)
                with source.open("rb") as incoming, destination.open("xb") as outgoing:
                    os.chmod(destination, 0o600)
                    shutil.copyfileobj(incoming, outgoing)
                    outgoing.flush()
                    os.fsync(outgoing.fileno())
            verify_backup(argparse.Namespace(dossier=dossier, package=args.package,
                          signing_key=signing_key, verification_key=public_key))
            args.receipt, args.receipt_signature = dossier / "restore-receipt.json", dossier / "restore-receipt.sig"
            deployment.apply()
            try:
                if services:
                    deployment.start_runtime_services(services)
                deployment.verify_runtime_health()
                completed = True
            except BaseException:
                if deployment.guarded_activation() is not None:
                    deployment.guarded_runner_stop()
                run(deployment.compose() + ["stop", "--timeout", "60", *WRITERS])
                raise
        except BaseException as error:
            record_failure(deployment, "local image update interrupted or refused", error)
            raise
        finally:
            # A failed frozen transaction still needs this private operation key
            # to verify its dossier and authorize explicit matched recovery.
            if completed:
                signing_key.unlink(missing_ok=True)
    print(f"Update complete; backup and journal: {deployment.stage}")


def start_existing(args):
    """Start retained containers only when no interrupted mutation needs recovery."""
    global COMMAND_DIAGNOSTICS
    args.stage = args.root / "backups" / datetime.now(timezone.utc).strftime("release-%Y%m%dT%H%M%SZ")
    deployment = Deployment(args)
    COMMAND_DIAGNOSTICS = deployment.root
    with deployment.lock():
        deployment.guard_recovery()
        deployment.plan = {"image_file": deployment.installed_image_file()}
        deployment.plan["compose_file"] = deployment.compose_path().name
        deployment.plan["active_overlay"] = deployment.active_overlay_input()
        services = run(deployment.compose() + ["ps", "--all", "--services"]).decode().splitlines()
        require({"backend", "frontend", "ansible-runner"} <= set(services), "installed application containers are missing")
        if deployment.guarded_activation() is None:
            run(deployment.compose() + ["start", "--wait", "--wait-timeout", "180", *services], timeout=210)
        else:
            deployment.guard_job_network_start()
            others = [service for service in services if service != "ansible-runner"]
            run(deployment.compose() + ["start", "--wait", "--wait-timeout", "180", *others], timeout=210)
            deployment.ensure_guarded_runner()
        run(deployment.compose() + ["exec", "-T", "backend", "curl", "--fail", "--silent",
                                   "--max-time", "10", "http://127.0.0.1:7000/api/v1/ready"], timeout=20)


def private_dossier_directory(path):
    directory = checked_path(path, directory=True)
    require(stat.S_IMODE(directory.stat().st_mode) == 0o700, "restore dossier must be private disk-backed scratch")
    filesystem = run(["findmnt", "--noheadings", "--output", "FSTYPE", "--target", str(directory)],
                     timeout=10, maximum=4096).decode().strip()
    require(filesystem and filesystem not in ("tmpfs", "ramfs", "devtmpfs"), "restore dossier must be disk-backed")
    return directory


def verify_backup(args):
    # Operator copies the private stage dossier and actual dump using existing
    # SSH/SCP. This action never contacts or executes commands on the client.
    directory = private_dossier_directory(args.dossier)
    try:
        return verify_backup_dossier(args, directory)
    except BaseException:
        # The projected/public boundary intentionally omits command output and
        # values. Preserve a private decisive failure for the authorized verifier.
        with contextlib.suppress(OSError, ValueError):
            write(directory / ("verify-boundary-" + uuid.uuid4().hex + ".private.log"), traceback.format_exc().encode())
        raise


def verify_backup_dossier(args, directory):
    package = verify_package(args.package, args.verification_key)
    plan = read_json(directory / "plan.json")
    request = read_json(directory / "backup-request.json")
    require(request["schema"] == "piping.compose-backup/v1"
            and request["helper_sha256"] == helper_identity() == plan["helper_sha256"]
            and request["plan_sha256"] == file_sha(directory / "plan.json")
            and request["package_sha256"] == file_sha(args.package / "package.json") == plan["package_sha256"]
            and plan["verification_key_sha256"] == file_sha(args.verification_key)
            and request["commit"] == package["commit"] == plan["commit"]
            and request["pipeline_id"] == package["pipeline_id"] == plan["pipeline_id"]
            and request["target"] == plan["target"]
            and request["root"] == plan["root"] and request["project_name"] == plan["project_name"]
            and re.fullmatch(r"[0-9a-f]{32}", request["nonce"]), "backup request identity mismatch")
    for name, description in request["files"].items():
        require(Path(name).name == name and file_sha(directory / name) == description["sha256"]
                and (directory / name).stat().st_size == description["size"], "copied backup bytes mismatch")
    required_files = {"offline-pre-upgrade.dump", "fingerprint.json", "proof-backend.env", "installed-target-schema.dump", "writers.json"}
    required_files.update(("runtime-inputs.json", "access-fence.json", "original-pg_hba.conf", "startup.compose.json"))
    preparation = request.get("preparation")
    require("preparation" in request and (preparation is None or (
        isinstance(preparation, dict) and set(preparation) == {"schema", "phase", "original_manifest_sha256"}
        and preparation["schema"] == "piping.fhrp-preparation/v1"
        and preparation["phase"] in ("original-r", "prepared-c")
        and preparation["original_manifest_sha256"] == file_sha(directory / "original-r-manifest.json"))),
        "invalid typed FHRP preparation binding")
    cutover = request.get("cutover")
    require("cutover" in request and (cutover is None or (
        preparation is None and isinstance(cutover, dict) and set(cutover) == {"schema", "phase"}
        and cutover["schema"] == "piping.automation-cutover/v1" and cutover["phase"] in ("original-r", "prepared-c"))),
        "invalid typed automation cutover binding")
    raw_cutover = cutover is not None and cutover["phase"] == "original-r"
    if raw_cutover:
        required_files.difference_update(("offline-pre-upgrade.dump", "fingerprint.json"))
    if preparation is None or preparation["phase"] == "prepared-c":
        kind = "cutover-r" if raw_cutover else "backup"
        required_files.add(kind + "-manifest.json")
        required_files.update(read_json(directory / (kind + "-manifest.json"))["files"])
    if cutover is not None and cutover["phase"] == "prepared-c":
        required_files.update(("cutover-r-manifest.json", "cutover-r-request.json", "cutover-r-receipt.json", "cutover-r-receipt.sig"))
        required_files.update(read_json(directory / "cutover-r-manifest.json")["files"])
    if preparation is not None:
        if preparation["phase"] == "original-r":
            required_files.difference_update(("offline-pre-upgrade.dump", "fingerprint.json"))
        else:
            required_files.update(("original-r-request.json", "original-r-receipt.json", "original-r-receipt.sig"))
        required_files.add("original-r-manifest.json")
        required_files.update(read_json(directory / "original-r-manifest.json")["files"])
    if not raw_cutover and (preparation is None or preparation["phase"] == "prepared-c") and pending_transforms(
            plan["prefix_manifest_sha384"], len(plan["installed_history"])):
        required_files.add("migration-expectation.json")
    if plan["preservation_sha256"]:
        required_files.add("preservation.json")
    if plan["check_capture_sha256"]:
        required_files.add("check-capture.json")
    require(set(request["files"]) == required_files,
            "incomplete backup dossier")
    require(request["preservation_sha256"] == plan["preservation_sha256"]
            == package.get("preservation", {}).get("manifest_sha256")
            and plan["check_capture_sha256"] == package.get("preservation", {}).get("capture_sha256")
            and plan["package_binding_sha256"] == package_binding(package)
            and file_sha(directory / "installed-target-schema.dump") == plan["schema_snapshot_sha256"],
            "backup preservation authority mismatch")
    require(type(request["database_bytes"]) is int and request["database_bytes"] > 0
            and shutil.disk_usage(directory).free >= max(request["database_bytes"] * 3, 1024 * 1024 * 1024),
            "insufficient leader disk capacity for isolated restore")
    require(plan["platform"] == package["platform"]
            and {role: {k: v for k, v in image.items() if k != "local_id"} for role, image in plan["images"].items()}
            == {role: {k: v for k, v in image.items() if k != "local_id"} for role, image in package["images"].items()},
            "backup image authority mismatch")
    plan["images"] = package["images"]
    deployment = Deployment.__new__(Deployment)
    deployment.args = argparse.Namespace(project_name="leader-proof-" + request["nonce"],
                                         proof_subnet=getattr(args, "proof_subnet", None))
    deployment.root, deployment.stage, deployment.plan = directory, directory, plan
    deployment.verifier = True
    # The verifier's explicit key is the authority, never a key supplied by the
    # copied dossier. Target-side validation uses the stage's pinned public key.
    deployment.verifier_key = args.verification_key
    deployment.controlled_writers()
    migrations, contract, contract_sha256 = deployment.published_migrations()
    require((migrations, contract, contract_sha256) == (plan["migrations"], plan["contract"], plan["contract_sha256"]),
            "backup migration source mismatch")
    validate_contract(contract, migrations)
    deployment.plan["postgres_image"] = load_image(directory, request["postgres_archive"], plan["platform"])
    save_authority(directory / request["postgres_archive"]["archive"], plan["target_postgres_image"],
                   request["postgres_archive"]["config_digest"])
    pending = len(plan["installed_history"]) < len(plan["migrations"])
    installed = None
    if pending:
        installed = load_image(directory, request["installed_backend_archive"], plan["platform"])
        save_authority(directory / request["installed_backend_archive"]["archive"], plan["target_backend_image"],
                       request["installed_backend_archive"]["config_digest"])
        deployment.plan["installed_backend_image"] = installed
    # Recompute the native schema oracle; client JSON does not authorize a schema.
    proof_environment = read_proof_environment(directory / "proof-backend.env")
    inputs = read_json(directory / "runtime-inputs.json")
    require(inputs["schema"] == "piping.deployment-runtime-inputs/v1"
            and inputs["environment_sha256"] == file_sha(directory / "proof-backend.env")
            and deployment.image_inputs(deployment.image("backend"), converter=True) == inputs["candidate"]
            and deployment.packaged_settings_preserved(inputs["installed"], inputs["candidate"])
            and (installed is None or deployment.image_inputs(installed, converter=False) == inputs["installed"])
            and inputs["effective_cipher_reference"] == proof_environment.get(
                "PIPING_SECRETS__CIPHER__KEY_MATERIAL_REF", inputs["candidate"]["cipher_reference"]),
            "copied paired executable/configuration inputs differ")
    reference = inputs["effective_cipher_reference"]
    require(isinstance(reference, str) and re.fullmatch(r"env:[A-Z_][A-Z_0-9]*", reference)
            and reference[4:] in proof_environment
            and inputs["effective_cipher_environment_sha256"] == sha(canonical({"reference": reference,
                "material": proof_environment[reference[4:]], "database_url": proof_environment["PIPING_DATABASE__URL"]})),
            "copied effective cipher environment binding differs")
    if preparation is not None:
        deployment.snapshot_manifest("original-r", authenticated=preparation["phase"] == "prepared-c")
    if preparation is None or preparation["phase"] == "prepared-c":
        deployment.snapshot_manifest("cutover-r" if raw_cutover else "backup", authenticated=False)
        recovery_images = read_json(directory / "recovery-images.json")
        for image in recovery_images.values():
            load_image(directory, image["archive"], plan["platform"])
            save_authority(directory / image["archive"]["archive"], image["image_id"], image["archive"]["config_digest"])
        require(recovery_images["backend"]["image_id"] == plan["target_backend_image"],
                "recovery backend differs from the observed source")
    if cutover is not None and cutover["phase"] == "prepared-c":
        deployment.snapshot_manifest("cutover-r")
    deployment.approval()
    expected_oracle = {key: plan[key] for key in ("content_type_seed_keys", "source_roundtrip_checks", "source_layout",
                      "installed_layout_slots", "source_raw_slots", "source_candidate_slots", "restored_target_catalog",
                      "restored_layout_slots", "installed_candidate_slots", "restored_candidate_slots",
                      "restored_candidate_catalog", "installed_candidate_additions", "installed_candidate_replacements",
                      "installed_candidate_removals")}
    deployment.service_image = lambda role: deployment.plan["postgres_image"] if role == "postgres" else installed
    deployment.reference_proof({"environment": proof_environment}, existing_environment=True)
    require(all(plan[key] == value for key, value in expected_oracle.items()), "client oracle differs from native source")
    preparation_proof = None
    cutover_proof = None
    if raw_cutover:
        require(deployment.workspace_cutover(), "original automation R does not target the exact workspace transition")
        cutover_proof = deployment.prove_cutover_r_request()
    elif preparation is not None and preparation["phase"] == "original-r":
        preparation_proof = {"schema": "piping.fhrp-preparation/v1", "semantics": deployment.prove_original_r_request()}
    else:
        baseline = read_json(directory / "fingerprint.json")
        require(baseline["history"] == plan["installed_history"], "backup ledger differs from staged history")
        if cutover is not None:
            # Re-evaluate the unchanged rows from the independently restored C,
            # never let the target's row assertion authorize retirement.
            with deployment.isolated_database("automation-prepared-c") as database:
                deployment.raw_restore(database, directory / "offline-pre-upgrade.dump", baseline)
                with deployment.database_target(database):
                    restored = deployment.fingerprint()
                    deployment.guard_cutover_conservation({**restored, "catalog": baseline["catalog"]})
        if deployment.transforms():
            deployment.load_migration_expectation(baseline)
        deployment.restore_proof(baseline)
    # Rehash after proof so a receipt cannot refer to bytes changed during restore.
    for name, description in request["files"].items():
        require(file_sha(directory / name) == description["sha256"], "backup bytes changed during proof")
    receipt = {"schema": "piping.compose-restore-receipt/v1", "request_sha256": file_sha(directory / "backup-request.json"),
                "request": request, "result": ("native-restore-conservation-passed" if raw_cutover
                                                  else "native-restore-migration-conservation-passed")}
    if preparation_proof is not None:
        receipt["preparation_proof"] = preparation_proof
    if cutover_proof is not None:
        receipt["cutover_proof"] = cutover_proof
    write_json(directory / "restore-receipt.json", receipt)
    sign(directory / "restore-receipt.json", args.signing_key, directory / "restore-receipt.sig")
    authenticate(directory / "restore-receipt.json", directory / "restore-receipt.sig", args.verification_key)


class DeployError(ValueError):
    """A public, secret-free deployment boundary failure."""


class RecoveryRequired(DeployError):
    pass


class ConcurrentOperation(DeployError):
    pass


def require(condition, message):
    if not condition:
        raise DeployError(message)


def checked_path(path, *, directory=False, missing=False):
    require(path.is_absolute() and path == Path(os.path.abspath(path)), "noncanonical path")
    for parent in reversed(path.parents):
        require(stat.S_ISDIR(parent.lstat().st_mode), "unsafe path ancestor")
    if missing:
        require(not os.path.lexists(path), "target already exists")
    else:
        mode = path.lstat().st_mode
        require(stat.S_ISDIR(mode) if directory else stat.S_ISREG(mode), "unsafe path type")
    return path


def read(path, maximum=MAX_BYTES):
    checked_path(path)
    require(path.stat().st_size <= maximum, "file exceeds boundary limit")
    return path.read_bytes()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def file_sha(path):
    checked_path(path)
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write(path, data):
    checked_path(path, missing=True)
    with path.open("xb") as stream:
        os.chmod(path, 0o600)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    sync_directory(path.parent)


def sync_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def write_json(path, value):
    write(path, json.dumps(value, sort_keys=True, separators=(",", ":")).encode())


def read_json(path):
    return json.loads(read(path), object_pairs_hook=reject_duplicate_json_keys)


def read_proof_environment(path):
    values = {}
    for line in read(path).decode().splitlines():
        key, separator, value = line.partition("=")
        require(separator and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key)
                and key not in values
                and key not in {"PIPING_MIGRATIONS_DIR", "LD_PRELOAD", "LD_LIBRARY_PATH"}
                and not any(c in value for c in "\r\n\x00"), "unsafe proof environment")
        values[key] = value
    return values


def environment():
    # Compose interpolation must come from the two explicit files, never the host.
    allowed = (
        "PATH",
        "HOME",
        "DOCKER_CONFIG",
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
        "DOCKER_TLS_VERIFY",
        "DOCKER_CERT_PATH",
        "DOCKER_TLS_CERTDIR",
        "XDG_RUNTIME_DIR",
    )
    return {key: os.environ[key] for key in allowed if key in os.environ}


def write_atomic(path, data, mode=0o600):
    """Replace one installation-owned file without exposing a partial write."""
    checked_path(path.parent, directory=True)
    temporary = path.parent / ("." + path.name + "." + uuid.uuid4().hex)
    write(temporary, data)
    os.chmod(temporary, mode)
    os.replace(temporary, path)
    sync_directory(path.parent)


def installed_compose_name(root, project_name, expected):
    """Bind the explicit input to any recorded installed container input."""
    observed = set()
    identifiers = run([
        "docker", "ps", "-aq", "--no-trunc",
        "--filter", "label=com.docker.compose.project=" + project_name,
    ]).decode().splitlines()
    for identifier in identifiers:
        inspected = json.loads(run(["docker", "inspect", identifier]))[0]
        labels = inspected["Config"].get("Labels") or {}
        files = labels.get("com.docker.compose.project.config_files", "").split(",")
        if not files or not files[0]:
            continue
        path = Path(files[0])
        if not path.is_absolute():
            path = root / path
        observed.add(path)
    if observed:
        require(len(observed) == 1, "installed containers disagree on the Compose input")
        path = next(iter(observed))
        require(path.parent == root and path.name == expected
                and path.is_file() and not path.is_symlink(), "unsupported installed Compose input")
        return path.name
    return expected


def runner_instance_unit(template, identifier):
    require(template.endswith("@.service") and re.fullmatch(r"[0-9a-f]{64}", identifier),
            "invalid guarded runner unit identity")
    return template[:-len("@.service")] + "@" + identifier + ".service"


COMPOSE_OPERATIONS = frozenset({"up", "down", "port", "exec", "ps", "logs", "stop", "version", "config"})
COMMAND_CATEGORIES = frozenset({"docker-compose-" + operation for operation in COMPOSE_OPERATIONS} | {
    "docker-compose-other", "docker-build", "docker-buildx-build", "docker-buildx-inspect", "docker-pull",
    "docker-image", "docker-other", "openssl", "python", "psql", "other"})
DIAGNOSTIC_TABLES = frozenset({*SERVICE_TABLES, SERVICE_MAPPING_TABLE, FHRP_TABLE, TAG_CATALOG_TABLE,
                               TAG_SELECTOR_TABLE, "public.inventory_content_type", "public.instance_branding",
                               "public.secrets_variable", "public.secrets_store", "public.secrets_binding",
                               "public.secret_usage_binding"})


def validated_failure_context(value):
    if not isinstance(value, dict):
        return {}
    category, status, state = value.get("command_category"), value.get("command_exit"), value.get("sqlstate")
    http_status = value.get("http_status")
    result = {}
    if type(category) is str and category in COMMAND_CATEGORIES:
        result["command_category"] = category
    if type(status) is int and -255 <= status <= 255:
        result["command_exit"] = status
    if category == "psql" and type(state) is str and re.fullmatch(r"[0-9A-Z]{5}", state):
        result["sqlstate"] = state
    if type(category) is str and category.startswith("docker-") and category in COMMAND_CATEGORIES \
            and type(http_status) is int and 400 <= http_status <= 599:
        result["http_status"] = http_status
    for name in ("pg_user_matches", "pg_database_matches", "pg_password_present", "pg_file_key_present",
                 "pg_invalid_keys_truncated", "mismatch_tables_truncated"):
        if type(value.get(name)) is bool:
            result[name] = value[name]
    keys = value.get("pg_invalid_keys")
    if type(keys) is list and len(keys) <= 16 and all(
            type(key) is str and re.fullmatch(r"PG[A-Z0-9_]{0,62}", key) for key in keys):
        result["pg_invalid_keys"] = keys
    tables = value.get("mismatch_tables")
    if type(tables) is list and len(tables) <= 16:
        accepted = []
        for item in tables:
            if (type(item) is not dict or set(item) != {"table", "expected_count", "actual_count"}
                    or type(item["table"]) is not str or item["table"] not in DIAGNOSTIC_TABLES
                    or any(count is not None and (type(count) is not int or not 0 <= count <= 1000000000)
                           for count in (item["expected_count"], item["actual_count"]))):
                break
            accepted.append(item)
        else:
            result["mismatch_tables"] = accepted
    return result


def command_failure_context(command, status, *, psql=False, stderr=b""):
    category = "other"
    if psql:
        category = "psql"
    elif command and command[0] == "docker":
        category = "docker-other"
        if list(command[1:3]) == ["buildx", "build"]:
            category = "docker-buildx-build"
        elif list(command[1:3]) == ["buildx", "imagetools"]:
            category = "docker-buildx-inspect"
        elif len(command) > 1 and command[1] in ("build", "pull", "image"):
            category = {"build": "docker-build", "pull": "docker-pull", "image": "docker-image"}[command[1]]
        elif len(command) > 1 and command[1] == "compose":
            category = "docker-compose-other"
            index = command.index("--profile") + 2 if "--profile" in command else 2
            if index < len(command) and command[index] in COMPOSE_OPERATIONS:
                category = "docker-compose-" + command[index]
    elif command and command[0] == "openssl":
        category = "openssl"
    elif command and re.fullmatch(r"python(?:3(?:\.[0-9]+)?)?", Path(command[0]).name):
        category = "python"
    context = {"command_category": category, "command_exit": status}
    if psql:
        states = set(re.findall(rb"(?:ERROR|FATAL|PANIC):\s+([0-9A-Z]{5}):", stderr))
        if len(states) == 1:
            context["sqlstate"] = states.pop().decode("ascii")
    elif category.startswith("docker-"):
        statuses = set(re.findall(rb"unexpected status(?: code)?(?: from [^\r\n]{0,512})?:\s*([45][0-9]{2})(?:\s|$)",
                                  stderr, re.IGNORECASE))
        statuses.update(re.findall(rb"unexpected HTTP status:\s*([45][0-9]{2})(?:\s|$)", stderr, re.IGNORECASE))
        if len(statuses) == 1:
            context["http_status"] = int(statuses.pop())
    return validated_failure_context(context)


def run(command, *, data=None, timeout=120, output=None, maximum=MAX_BYTES, psql=False):
    require(0 < timeout <= 300, "invalid command deadline")
    # Private captured output is never included in exceptions or public summaries.
    with tempfile.TemporaryFile() as captured, tempfile.TemporaryFile() as errors:
        result = subprocess.run(
            command,
            input=data,
            stdout=output or captured,
            stderr=errors,
            timeout=timeout,
            env=environment(),
            check=False,
        )
        if result.returncode:
            errors.seek(0)
            private_error = errors.read(MAX_BYTES)
        if result.returncode and COMMAND_DIAGNOSTICS is not None:
            # Keep native stderr private; never place it in public exceptions.
            with contextlib.suppress(OSError, ValueError):
                checked_path(COMMAND_DIAGNOSTICS, directory=True)
                require(stat.S_IMODE(COMMAND_DIAGNOSTICS.stat().st_mode) == 0o700,
                        "command diagnostics require a private stage")
                write(COMMAND_DIAGNOSTICS / ("native-command-" + uuid.uuid4().hex + ".log"),
                      private_error)
        if result.returncode:
            error = DeployError("native command failed; private output suppressed")
            error.native_failure_context = command_failure_context(command, result.returncode, psql=psql,
                                                                   stderr=private_error)
            raise error
        if output is not None:
            return b""
        require(captured.tell() <= maximum, "native output exceeds boundary limit")
        captured.seek(0)
        return captured.read()


def render_images(source, refs):
    lines, seen, keys = [], set(), set()
    for line in source.splitlines(keepends=True):
        match = re.match(rb"[ \t]*(?:export[ \t]+)?([A-Za-z_][A-Za-z0-9_]*)[ \t]*=", line)
        if match:
            require(match[1] not in keys, "ambiguous duplicate environment variable")
            keys.add(match[1])
        if match and match[1].decode() in ROLES.values():
            key = match[1].decode()
            require(key not in seen, "ambiguous duplicate image variable")
            seen.add(key)
            role = next(role for role, variable in ROLES.items() if variable == key)
            ending = b"\r\n" if line.endswith(b"\r\n") else b"\n" if line.endswith(b"\n") else b""
            # Keep spacing and inline comments; reject multiline/quoted assignments.
            suffix = line[match.end() :].removesuffix(ending)
            old = re.fullmatch(rb"[^\s\"'`#]+([ \t]*(?:#[^\r\n]*)?)", suffix)
            if old is None:
                raise DeployError("ambiguous image assignment")
            line = line[: match.end()] + refs[role].encode() + (old[1] or b"") + ending
        lines.append(line)
    require(seen == set(ROLES.values()), "three explicit image variables required")
    return b"".join(lines)


def migration_artifact(archive):
    require(len(archive) <= MAX_BYTES, "migration archive exceeds boundary limit")
    found, paths, contract, contract_sha256 = {}, set(), None, None
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as bundle:
        for item in bundle:
            path = PurePosixPath(item.name)
            require(
                not path.is_absolute()
                and ".." not in path.parts
                and path.parts
                and path.parts[0] == "migrations"
                and str(path) not in paths,
                "unsafe migration archive path",
            )
            paths.add(str(path))
            require(item.isdir() or item.isfile(), "migration archive links refused")
            if item.isdir():
                require(len(path.parts) == 1, "unexpected migration directory")
                continue
            require(
                len(path.parts) == 2 and 0 <= item.size <= 2 * 1024 * 1024,
                "unexpected migration file",
            )
            # Docker packages the owner's documentation and empty directory marker too.
            if path.name == "README.md" or (path.name == ".keep" and item.size == 0):
                continue
            if path.name == "maintenance-transition.json":
                # Operator-only source metadata is packaged with migrations.
                # Image-carried bytes never authorize ledger repair; deployment
                # uses only its sealed compose contract and actual SQL checksums.
                continue
            if path.name == "compose-deployment.json":
                stream = bundle.extractfile(item)
                require(stream is not None, "migration contract missing")
                content = stream.read(item.size + 1)
                require(len(content) == item.size, "truncated migration contract")
                contract = json.loads(content, object_pairs_hook=reject_duplicate_json_keys)
                contract_sha256 = sha(content)
                continue
            match = re.fullmatch(r"([0-9]{4})_[A-Za-z0-9_]+\.sql", path.name)
            if match is None:
                raise DeployError("unexpected migration file")
            version = int(match[1])
            require(version not in found, "duplicate migration version")
            stream = bundle.extractfile(item)
            if stream is None:
                raise DeployError("migration file missing")
            content = stream.read(item.size + 1)
            require(len(content) == item.size, "truncated migration file")
            found[version] = hashlib.sha384(content).hexdigest()
    require(
        bool(found) and all(version == ordinal for ordinal, version in enumerate(sorted(found), 1)),
        "published migrations must be a nonempty contiguous history starting at 1",
    )
    return {str(version): checksum for version, checksum in sorted(found.items())}, contract, contract_sha256


def migration_checksums(archive):
    return migration_artifact(archive)[0]


def source_prefix_manifest(archive):
    """Native proof authority uses immutable source paths, not ledger versions."""
    checksums = migration_artifact(archive)[0]
    result = {}
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as bundle:
        for item in bundle:
            if item.isfile() and item.name.endswith(".sql"):
                name = PurePosixPath(item.name).name
                version = str(int(name.split("_", 1)[0]))
                result["apps/api/migrations/" + name] = checksums[version]
    return result


def validate_contract(contract, migrations):
    validate_baseline_layout_contract(contract)
    require(
        isinstance(migrations, dict)
        # Canonical stage JSON sorts object keys lexically, not by migration number.
        and set(migrations) == {str(v) for v in range(1, len(migrations) + 1)}
        and migrations.get("1") == BASELINE_SQLX_SHA384
        and all(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{96}", value)
                for value in migrations.values()),
        "unsupported source-bound migration contract",
    )
    return contract


def baseline_positions(contract, canonical_catalog, installed_catalog):
    """Accept only the two source-proven complete baseline physical layouts.

    Full24 leaves dropped credential-column slots; a fresh native baseline is
    compact. The sealed source pairs bind that complete layout, not arbitrary
    positions or historical CHECK approvals. All other catalog fields stay exact.
    """
    pairs = contract["position_pairs"]
    columns = lambda catalog: {o["name"]: o["definition"]["position"]
                               for o in catalog if o["kind"] == "column"}
    fresh = columns(canonical_catalog)
    require(all(fresh.get(name) == pair[0] for name, pair in pairs.items()),
            "source baseline layout differs from sealed native proof")
    return baseline_layout_positions(contract, installed_catalog)


def baseline_append_slots(canonical_catalog, source_slots, positions):
    """Translate only a sealed terminal column, never infer target append slots."""
    terminal = {}
    for obj in canonical_catalog:
        if obj["kind"] != "column":
            continue
        table = obj["name"].rsplit(".", 1)[0]
        column = (obj["definition"]["position"], obj["name"])
        if table not in terminal or column[0] > terminal[table][0]:
            terminal[table] = column
    expected = dict(source_slots)
    for table, slot in source_slots.items():
        if table not in terminal and slot == 0:
            continue
        require(table in terminal, "source append table has no canonical columns")
        position, name = terminal[table]
        if name in positions:
            before, after = positions[name]
            require(slot == position == before,
                    "source append slot differs from the sealed terminal column")
            expected[table] = after
    return expected


def compacted_layout_slots(catalog):
    slots = {obj["name"]: 0 for obj in catalog if obj["kind"] == "relation"
             and obj["definition"]["kind"] in ("r", "p", "v", "m", "f")}
    for obj in catalog:
        if obj["kind"] == "column":
            table = obj["name"].rsplit(".", 1)[0]
            require(table in slots, "column has no native relation")
            slots[table] = max(slots[table], obj["definition"]["position"])
    return slots


# Closed source relation set whose physical attribute order follows the replay
# baseline: 0003_service_port_mappings.sql and 0004_fhrp_auth_key_secret.sql
# create the historical attributes and then drop them, so the canonical replay
# keeps attribute slots these relations never carried in a pre-baseline database.
# The live definitions stay canonical; only these relations' positions follow
# the same source replay order compactly. No target input can extend this list.
SOURCE_REBUILT_RELATIONS = ("public.ipam_fhrpgroup", "public.ipam_service",
                            "public.ipam_servicetemplate")


def source_rebuilt_layout(catalog, slots):
    """Complete source-derived layout for the closed baseline-rebuilt relations.

    Positions come only from the native canonical replay order; terminal slots
    are the same order. Every other relation keeps its exact canonical attnums
    and slots.
    """
    require(isinstance(catalog, list) and isinstance(slots, dict), "invalid source layout input")
    relations = set(SOURCE_REBUILT_RELATIONS)
    present = {obj["name"] for obj in catalog if obj.get("kind") == "relation"
               and obj["name"] in relations and obj["definition"].get("kind") in ("r", "p")}
    require(present == relations, "source rebuilt relation set differs from the closed list")
    rebuilt = json.loads(json.dumps(catalog))
    compacted = {}
    for obj in rebuilt:
        if obj.get("kind") == "column":
            compacted.setdefault(obj["name"].rsplit(".", 1)[0], []).append(obj)
    rebuilt_slots = dict(slots)
    for relation in SOURCE_REBUILT_RELATIONS:
        columns = compacted.get(relation)
        require(isinstance(columns, list) and columns, "source rebuilt relation has no canonical columns")
        for position, obj in enumerate(sorted(columns, key=lambda item: item["definition"]["position"]), 1):
            require(type(obj["definition"]["position"]) is int, "invalid canonical column position")
            obj["definition"]["position"] = position
        require(relation in rebuilt_slots, "source rebuilt relation has no native terminal slot")
        rebuilt_slots[relation] = len(columns)
    return rebuilt, rebuilt_slots


def monitoring_projection_queries(*, before):
    """Project 0020 rows onto source identities, not randomly generated UUIDs."""
    seeds = ("seed(name,slug,exporter_slugs) AS (VALUES "
             "('Linux baseline','linux-baseline',ARRAY['node','process','smartctl']::text[]),"
             "('Docker host','docker-host',ARRAY['node','docker']::text[]),"
             "('Web nginx','web-nginx',ARRAY['nginx']::text[]),"
             "('Postgres','postgres',ARRAY['postgres']::text[]),"
             "('Network blackbox','network-blackbox',ARRAY['blackbox-interface']::text[]),"
             "('K8s node','k8s-node',ARRAY['node','docker']::text[]))")
    if before:
        name = "coalesce(nullif(btrim(a.name),''),e.name)"
        slug = ("left(coalesce(nullif(btrim(regexp_replace(regexp_replace(lower(" + name
                + "),'[^a-z0-9]+','-','g'),'(^-+)|(-+$)','','g'),'-'),''),'profile'),95)"
                " || '-' || replace(a.id::text,'-','')")
        legacy = ("legacy AS (SELECT a.*, " + name + " AS profile_name, " + slug
                  + " AS profile_slug FROM public.monitoring_exporter_assignment a "
                    "JOIN public.monitoring_exporter e ON e.id=a.exporter_id)")
        prefix = "WITH " + seeds + ", " + legacy + " "
        profiles = ("SELECT jsonb_build_array('assignment:'||id::text,profile_name,profile_slug,"
                    "NULL,NULL,NULL,NULL,true) AS payload FROM legacy UNION ALL "
                    "SELECT jsonb_build_array('seed:'||s.slug,s.name,s.slug,NULL,NULL,NULL,NULL,true) "
                    "FROM seed s WHERE EXISTS (SELECT FROM public.monitoring_exporter e "
                    "WHERE e.slug=ANY(s.exporter_slugs))")
        items = ("SELECT jsonb_build_array('assignment:'||id::text,exporter_id,job,scheme,port,"
                 "metrics_path,labels,true,0) AS payload FROM legacy UNION ALL "
                 "SELECT jsonb_build_array('seed:'||s.slug,e.id,NULL,NULL,NULL,NULL,'{}'::jsonb,true,"
                 "(array_position(s.exporter_slugs,e.slug)-1)::integer) FROM seed s "
                 "JOIN public.monitoring_exporter e ON e.slug=ANY(s.exporter_slugs)")
        assignments = ("SELECT jsonb_build_array(id,'assignment:'||id::text,scope_type,scope_id,"
                       "weight,enabled,target) AS payload FROM legacy")
    else:
        # A profile is either attached to its preserved assignment or identified
        # by its seed slug. Cardinality and UUID/timestamp checks run separately.
        prefix = ("WITH profile_key AS (SELECT p.*,CASE WHEN a.id IS NULL THEN 'seed:'||p.slug "
                  "ELSE 'assignment:'||a.id::text END AS conserved_key "
                  "FROM public.monitoring_profile p LEFT JOIN public.monitoring_profile_assignment a "
                  "ON a.profile_id=p.id) ")
        profiles = ("SELECT jsonb_build_array(conserved_key,name,slug,description,metric_set_id,"
                    "query_source_id,alert_source_id,enabled) AS payload FROM profile_key")
        items = ("SELECT jsonb_build_array(p.conserved_key,i.exporter_id,i.job,i.scheme,i.port,"
                 "i.metrics_path,i.labels,i.enabled,i.sort_order) AS payload "
                 "FROM public.monitoring_profile_item i JOIN profile_key p ON p.id=i.profile_id")
        assignments = ("SELECT jsonb_build_array(a.id,p.conserved_key,a.scope_type,a.scope_id,"
                       "a.weight,a.enabled,a.target) AS payload FROM public.monitoring_profile_assignment a "
                       "JOIN profile_key p ON p.id=a.profile_id")
    return dict(zip(migration_contract.MONITORING_NEW_TABLES,
                    (prefix + profiles, prefix + items, prefix + assignments), strict=True))


def monitoring_digest_query(projection):
    return ("SELECT count(*)::text || ':' || "
            "encode(sha256(convert_to(coalesce(string_agg(h,'' ORDER BY h),''),'UTF8')),'hex') "
            "FROM (SELECT encode(sha256(convert_to(payload::text,'UTF8')),'hex') AS h FROM ("
            + projection + ") projected) hashed")



def exported_config(ref, expected, directory):
    # Inspect.Id addresses a manifest in containerd stores, not necessarily its
    # config. Verify the engine's actual config bytes through native image export.
    with tempfile.TemporaryFile(dir=directory) as archive:
        run(["docker", "image", "save", ref], output=archive, timeout=300)
        archive.seek(0)
        config_path, config, descriptions = None, None, None
        with tarfile.open(fileobj=archive, mode="r|") as bundle:
            for item in bundle:
                path = PurePosixPath(item.name)
                require(
                    not path.is_absolute() and ".." not in path.parts and (item.isdir() or item.isfile()),
                    "unsafe native image export",
                )
                if not item.isfile() or item.size > 1024 * 1024:
                    continue
                stream = bundle.extractfile(item)
                if stream is None:
                    raise DeployError("missing native image export member")
                content = stream.read(item.size + 1)
                require(len(content) == item.size, "truncated native image export")
                if "sha256:" + sha(content) == expected:
                    require(config_path is None, "ambiguous exported image config")
                    config_path = item.name
                    config = json.loads(content, object_pairs_hook=reject_duplicate_json_keys)
                if item.name == "manifest.json":
                    require(descriptions is None, "ambiguous native export manifest")
                    descriptions = json.loads(content, object_pairs_hook=reject_duplicate_json_keys)
        require(
            isinstance(descriptions, list)
            and len(descriptions) == 1
            and config_path is not None
            and descriptions[0]["Config"] == config_path,
            "native image export is not the published config",
        )
        archive.seek(0)
        platform = "/".join(filter(None, (config["os"], config["architecture"], config.get("variant", ""))))
        save_identity(archive, expected, platform)
        return config


DATAFLOW_FLOW_STATE_SQL = """
SELECT coalesce(jsonb_agg(jsonb_build_object(
 'id',f.id::text,'name',f.name,'description',f.description,'status',f.status,
 'transport',f.transport,'destination_ports',to_jsonb(f.destination_ports),
 'allow_cross_vrf',f.allow_cross_vrf) ORDER BY f.id),'[]'::jsonb)
FROM public.inventory_dataflow f;
"""

DATAFLOW_ENDPOINT_STATE_PRE_SQL = """
SELECT coalesce(jsonb_agg(entry ORDER BY entry->>'id', entry->>'side'),'[]'::jsonb) FROM (
 SELECT jsonb_build_object('id',f.id::text,'side','source','kind',f.source_kind,'mode',f.source_mode,
  'reference',CASE f.source_kind
   WHEN 'dcim.device' THEN f.source_device_id::text
   WHEN 'virtualization.virtualmachine' THEN f.source_vm_id::text
   WHEN 'ipam.service' THEN f.source_service_id::text
   WHEN 'ipam.ipaddress' THEN f.source_ipaddress_id::text
   WHEN 'ipam.prefix' THEN f.source_prefix_id::text
   WHEN 'dcim.region' THEN f.source_region_id::text
   WHEN 'dcim.site' THEN f.source_site_id::text
   WHEN 'dcim.sitegroup' THEN f.source_sitegroup_id::text
   WHEN 'dcim.devicerole' THEN f.source_devicerole_id::text
   WHEN 'tenancy.tenant' THEN f.source_tenant_id::text
   WHEN 'tenancy.tenantgroup' THEN f.source_tenantgroup_id::text
   WHEN 'dcim.platform' THEN f.source_platform_id::text
   WHEN 'dcim.manufacturer' THEN f.source_manufacturer_id::text
   WHEN 'dcim.devicetype' THEN f.source_devicetype_id::text
   WHEN 'dcim.rack' THEN f.source_rack_id::text
   WHEN 'dcim.location' THEN f.source_location_id::text
   WHEN 'virtualization.cluster' THEN f.source_cluster_id::text
   WHEN 'virtualization.clustergroup' THEN f.source_clustergroup_id::text
   WHEN 'virtualization.clustertype' THEN f.source_clustertype_id::text
   ELSE NULL END) AS entry
 FROM public.inventory_dataflow f
 UNION ALL
 SELECT jsonb_build_object('id',f.id::text,'side','destination','kind',f.destination_kind,'mode',f.destination_mode,
  'reference',CASE f.destination_kind
   WHEN 'dcim.device' THEN f.destination_device_id::text
   WHEN 'virtualization.virtualmachine' THEN f.destination_vm_id::text
   WHEN 'ipam.service' THEN f.destination_service_id::text
   WHEN 'ipam.ipaddress' THEN f.destination_ipaddress_id::text
   WHEN 'ipam.prefix' THEN f.destination_prefix_id::text
   WHEN 'dcim.region' THEN f.destination_region_id::text
   WHEN 'dcim.site' THEN f.destination_site_id::text
   WHEN 'dcim.sitegroup' THEN f.destination_sitegroup_id::text
   WHEN 'dcim.devicerole' THEN f.destination_devicerole_id::text
   WHEN 'tenancy.tenant' THEN f.destination_tenant_id::text
   WHEN 'tenancy.tenantgroup' THEN f.destination_tenantgroup_id::text
   WHEN 'dcim.platform' THEN f.destination_platform_id::text
   WHEN 'dcim.manufacturer' THEN f.destination_manufacturer_id::text
   WHEN 'dcim.devicetype' THEN f.destination_devicetype_id::text
   WHEN 'dcim.rack' THEN f.destination_rack_id::text
   WHEN 'dcim.location' THEN f.destination_location_id::text
   WHEN 'virtualization.cluster' THEN f.destination_cluster_id::text
   WHEN 'virtualization.clustergroup' THEN f.destination_clustergroup_id::text
   WHEN 'virtualization.clustertype' THEN f.destination_clustertype_id::text
   ELSE NULL END) AS entry
 FROM public.inventory_dataflow f
) rows;
"""

DATAFLOW_ENDPOINT_STATE_POST_SQL = """
SELECT coalesce(jsonb_agg(entry ORDER BY entry->>'id', entry->>'side'),'[]'::jsonb) FROM (
 SELECT jsonb_build_object('id',f.id::text,'side',s.side,'kind',s.object_type,'mode',s.address_mode,
  'reference',CASE s.object_type
   WHEN 'dcim.device' THEN s.device_id::text
   WHEN 'virtualization.virtualmachine' THEN s.vm_id::text
   WHEN 'ipam.service' THEN s.service_id::text
   WHEN 'ipam.ipaddress' THEN s.ipaddress_id::text
   WHEN 'ipam.prefix' THEN s.prefix_id::text
   WHEN 'ipam.iprange' THEN s.iprange_id::text
   WHEN 'extras.tag' THEN s.tag_id::text
   WHEN 'inventory.addressgroup' THEN s.addressgroup_id::text
   WHEN 'dcim.region' THEN s.region_id::text
   WHEN 'dcim.site' THEN s.site_id::text
   WHEN 'dcim.sitegroup' THEN s.sitegroup_id::text
   WHEN 'dcim.devicerole' THEN s.devicerole_id::text
   WHEN 'tenancy.tenant' THEN s.tenant_id::text
   WHEN 'tenancy.tenantgroup' THEN s.tenantgroup_id::text
   WHEN 'dcim.platform' THEN s.platform_id::text
   WHEN 'dcim.manufacturer' THEN s.manufacturer_id::text
   WHEN 'dcim.devicetype' THEN s.devicetype_id::text
   WHEN 'dcim.rack' THEN s.rack_id::text
   WHEN 'dcim.location' THEN s.location_id::text
   WHEN 'virtualization.cluster' THEN s.cluster_id::text
   WHEN 'virtualization.clustergroup' THEN s.clustergroup_id::text
   WHEN 'virtualization.clustertype' THEN s.clustertype_id::text
   ELSE NULL END) AS entry
 FROM public.inventory_dataflow f
 JOIN public.inventory_dataflow_selector s ON s.dataflow_id=f.id
) rows;
"""

DATAFLOW_SELECTED_STATE_PRE_SQL = """
SELECT coalesce(jsonb_agg(jsonb_build_array(dataflow_id::text,side,ipaddress_id::text)
 ORDER BY dataflow_id::text,side,ipaddress_id::text),'[]'::jsonb)
FROM public.inventory_dataflow_selected_ip;
"""

DATAFLOW_SELECTED_STATE_POST_SQL = """
SELECT coalesce(jsonb_agg(jsonb_build_array(s.dataflow_id::text,s.side,n.ipaddress_id::text)
 ORDER BY s.dataflow_id::text,s.side,n.ipaddress_id::text),'[]'::jsonb)
FROM public.inventory_dataflow_selector_ip n
JOIN public.inventory_dataflow_selector s ON s.id=n.selector_id
WHERE s.dataflow_id IS NOT NULL;
"""

DATAFLOW_CONTENT_TYPE_KEYS_SQL = """
SELECT coalesce(jsonb_agg(jsonb_build_array(app_label,model) ORDER BY app_label,model),'[]'::jsonb)
FROM public.inventory_content_type;
"""


class Deployment:
    def __init__(self, args):
        self.args, self.root, self.stage = args, args.root, args.stage
        self.history_verification_key = getattr(args, "verification_key", None)
        checked_path(self.root, directory=True)
        require(
            len(self.root.parts) >= 3 and self.root not in (Path.home(), Path("/usr/local")),
            "generic root directory refused",
        )
        require(
            self.root.parts[1] not in ("proc", "sys", "dev", "etc", "usr", "bin", "sbin", "boot"),
            "system root hierarchy refused",
        )
        require(
            self.stage.parent == self.root / "backups" and re.fullmatch(r"release-[0-9]{8}T[0-9]{6}Z", self.stage.name),
            "unsafe stage path",
        )
        datetime.strptime(self.stage.name, "release-%Y%m%dT%H%M%SZ").replace(tzinfo=timezone.utc)
        require(
            re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,62}", args.project_name),
            "invalid Compose project",
        )
        compose_file = getattr(args, "compose_file", COMPOSE_FILE_DEFAULT)
        require(
            isinstance(compose_file, str)
            and Path(compose_file).name == compose_file
            and compose_file not in (".env", ".images.env")
            and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}", compose_file),
            "invalid installed Compose filename",
        )
        compose_path = checked_path(self.root / compose_file)
        require(
            stat.S_ISREG(compose_path.lstat().st_mode)
            and compose_path.lstat().st_nlink == 1,
            "installed Compose file must be a regular non-symlink file",
        )
        self.compose_file = compose_file
        self.plan: dict[str, Any] = {}

    def compose_path(self):
        name = self.plan.get("compose_file")
        if not name:
            name = installed_compose_name(self.root, self.args.project_name, self.compose_file)
            self.plan["compose_file"] = name
        require(name == self.compose_file, "installed Compose input differs from explicit input")
        return self.root / name

    def config_names(self):
        return [ENV_FILE, self.compose_path().name]

    def installed_image_file(self):
        """Select the one installed image-assignment owner; never guess between two."""
        def declares(path):
            try:
                content = read(path)
            except OSError:
                raise DeployError("installed image assignment is inaccessible") from None
            keys = {match[1] for line in content.splitlines()
                    if (match := re.match(rb"[ \t]*(?:export[ \t]+)?(PIPING_(?:BACKEND|FRONTEND|ANSIBLE_RUNNER)_IMAGE)[ \t]*=", line))}
            return bool(keys)

        candidates = []
        for name in (".images.env", ENV_FILE):
            path = self.root / name
            try:
                info = os.lstat(path)
            except FileNotFoundError:
                continue
            except OSError:
                raise DeployError("installed image assignment is inaccessible") from None
            require(stat.S_ISREG(info.st_mode) and not stat.S_ISLNK(info.st_mode),
                    "unsafe installed image assignment")
            if declares(path):
                candidates.append(name)
        require(len(candidates) == 1, "ambiguous or missing installed image assignment owner")
        return candidates[0]

    def job_network_directory(self):
        return self.root / JOB_NETWORK_DIRECTORY

    @staticmethod
    def guarded_overlay_bytes(activation):
        network = activation["network"]
        require(re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}", network),
                "invalid guarded job network name")
        return ("services:\n"
                "  ansible-runner:\n"
                "    restart: \"no\"\n"
                "    environment:\n"
                "      PIPING_SANDBOX_DOCKER_NETWORK: \"" + network + "\"\n").encode()

    def guarded_activation(self, *, allow_incomplete=False):
        """Read the one installation-owned guarded-network startup contract."""
        if hasattr(self, "_guarded_activation"):
            return self._guarded_activation
        directory = self.job_network_directory()
        try:
            directory_info = os.lstat(directory)
        except FileNotFoundError:
            self._guarded_activation = None
            return None
        except OSError:
            raise DeployError("guarded job network state is inaccessible") from None
        require(stat.S_ISDIR(directory_info.st_mode) and not stat.S_ISLNK(directory_info.st_mode)
                and stat.S_IMODE(directory_info.st_mode) == 0o700
                and directory_info.st_uid == 0, "unsafe guarded job network directory")
        path = directory / JOB_NETWORK_ACTIVATION_FILE
        try:
            path_info = os.lstat(path)
        except FileNotFoundError:
            marker_path = directory / JOB_NETWORK_DEACTIVATION_FILE
            try:
                marker_info = os.lstat(marker_path)
            except FileNotFoundError:
                require(allow_incomplete, "guarded job network activation state is incomplete")
                require(not os.path.lexists(directory / JOB_NETWORK_OVERLAY),
                        "orphaned guarded Compose overlay")
                self._guarded_activation = None
                return None
            except OSError:
                raise DeployError("guarded job network deactivation state is inaccessible") from None
            require(stat.S_ISREG(marker_info.st_mode) and not stat.S_ISLNK(marker_info.st_mode)
                    and stat.S_IMODE(marker_info.st_mode) == 0o600
                    and marker_info.st_uid == 0, "unsafe job network deactivation state")
            marker = read_json(marker_path)
            require(marker == {"schema": "piping.job-network-deactivated/v1",
                               "root": str(self.root), "project_name": self.args.project_name}
                    and not os.path.lexists(directory / JOB_NETWORK_OVERLAY),
                    "invalid job network deactivation state")
            self._guarded_activation = None
            return None
        except OSError:
            raise DeployError("guarded job network activation state is inaccessible") from None
        require(not os.path.lexists(directory / JOB_NETWORK_DEACTIVATION_FILE),
                "conflicting guard activation and deactivation states")
        require(stat.S_ISREG(path_info.st_mode) and not stat.S_ISLNK(path_info.st_mode)
                and stat.S_IMODE(path_info.st_mode) == 0o600
                and path_info.st_uid == 0, "unsafe guarded job network activation")
        value = read_json(path)
        keys = {"schema", "root", "project_name", "network", "bridge", "compose_file",
                "guard_unit", "policy_unit", "runner_unit", "foundation_comment", "restart"}
        require(set(value) == keys and value["schema"] == JOB_NETWORK_ACTIVATION
                and value["root"] == str(self.root) and value["project_name"] == self.args.project_name
                and value["restart"] == "no"
                and value["compose_file"] == self.compose_file
                and value["compose_file"] == self.compose_path().name
                and re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}", value["network"])
                and re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,14}", value["bridge"])
                and value["foundation_comment"] == "piping-job-network:" + str(self.root) + ":v1"
                and re.fullmatch(r"[a-z0-9][a-z0-9_.@-]{0,127}-guard\.service", value["guard_unit"])
                and re.fullmatch(r"[a-z0-9][a-z0-9_.@-]{0,127}-policy\.service", value["policy_unit"])
                and re.fullmatch(r"[a-z0-9][a-z0-9_.@-]{0,127}-runner@\.service", value["runner_unit"]),
                "invalid guarded job network activation")
        overlay = directory / JOB_NETWORK_OVERLAY
        require(overlay.is_file() and not overlay.is_symlink()
                and stat.S_IMODE(overlay.stat().st_mode) == 0o600
                and overlay.stat().st_uid == 0
                and read(overlay) == self.guarded_overlay_bytes(value),
                "guarded Compose overlay changed")
        self._guarded_activation = value
        return value

    def guarded_compose_path(self, *, allow_incomplete=False):
        active = self.guarded_activation(allow_incomplete=allow_incomplete)
        return self.job_network_directory() / JOB_NETWORK_OVERLAY if active else None

    def job_network_identity(self, activation):
        directory = self.job_network_directory()
        path = directory / JOB_NETWORK_IDENTITY_FILE
        require(path.is_file() and not path.is_symlink()
                and stat.S_IMODE(path.stat().st_mode) == 0o600
                and path.stat().st_uid == 0, "unsafe job network identity file")
        identity = read_json(path)
        require(set(identity) == {"Id", "Name", "Driver", "Scope", "Internal", "Attachable", "Ingress",
                                  "EnableIPv6", "IPAM", "Options", "Labels"}
                and identity["Name"] == activation["network"]
                and re.fullmatch(r"[0-9a-f]{64}", identity["Id"])
                and identity["Driver"] == "bridge" and identity["Internal"] is False
                and identity["EnableIPv6"] is False
                and identity["Options"].get("com.docker.network.bridge.name") == activation["bridge"]
                and identity["Labels"].get("space.piping.owner") == str(directory),
                "job network identity contract mismatch")
        return identity

    def unit_state(self, unit):
        query = unit
        if unit.endswith("@.service"):
            query = unit[:-len("@.service")] + "@" + "a" * 64 + ".service"
        output = run(["systemctl", "show", "--property=LoadState", "--property=ActiveState",
                      "--property=SubState", query]).decode()
        values = {}
        for line in output.splitlines():
            key, separator, value = line.partition("=")
            require(separator and key not in values, "invalid native unit state")
            values[key] = value
        require({"LoadState", "ActiveState", "SubState"} <= set(values),
                "native unit state unavailable")
        return values

    def stop_runner_unit(self, unit, *, disable, check_container):
        """Stop a loaded runner unit in any transitional state and prove quiescence."""
        state = self.unit_state(unit)
        if state["LoadState"] != "loaded":
            if check_container:
                identifier = self.runner_identifier()
                if identifier is not None:
                    inspected = json.loads(run(["docker", "inspect", identifier]))[0]
                    if inspected["State"]["Running"]:
                        run(["docker", "stop", "--time", "20", identifier], timeout=60)
                        inspected = json.loads(run(["docker", "inspect", identifier]))[0]
                    require(inspected["State"]["Running"] is False,
                            "guarded runner container remains after unit stop")
            return
        run(["systemctl", "stop", unit], timeout=60)
        deadline = time.monotonic() + 15
        while True:
            state = self.unit_state(unit)
            if state["LoadState"] != "loaded" or (
                    state["ActiveState"] in ("inactive", "failed")
                    and state["SubState"] in ("dead", "failed")):
                break
            require(time.monotonic() < deadline, "guarded runner unit did not quiesce")
            time.sleep(0.1)
        if self.unit_state(unit)["LoadState"] == "loaded":
            subprocess.run(["systemctl", "reset-failed", unit], env=environment(),
                           timeout=30, check=False, capture_output=True)
        state = self.unit_state(unit)
        require(state["LoadState"] != "loaded"
                or (state["ActiveState"] in ("inactive", "failed")
                    and state["SubState"] in ("dead", "failed")),
                "guarded runner unit still has a pending start")
        if disable:
            run(["systemctl", "disable", unit], timeout=60)
        if check_container:
            identifier = self.runner_identifier()
            if identifier is not None:
                inspected = json.loads(run(["docker", "inspect", identifier]))[0]
                if inspected["State"]["Running"]:
                    run(["docker", "stop", "--time", "20", identifier], timeout=60)
                    inspected = json.loads(run(["docker", "inspect", identifier]))[0]
                require(inspected["State"]["Running"] is False,
                        "guarded runner container remains after unit stop")

    def recorded_runner_unit(self):
        path = self.job_network_directory() / JOB_NETWORK_RUNNER_UNIT_FILE
        if not os.path.lexists(path):
            return None
        require(path.is_file() and not path.is_symlink()
                and stat.S_IMODE(path.stat().st_mode) == 0o600
                and path.stat().st_uid == 0, "unsafe recorded guarded runner unit")
        unit = path.read_text().strip()
        require(re.fullmatch(r"[a-z0-9][a-z0-9_.@-]{0,127}@[0-9a-f]{64}\.service", unit),
                "invalid recorded guarded runner unit")
        return unit

    def write_recorded_runner_unit(self, unit):
        require(re.fullmatch(r"[a-z0-9][a-z0-9_.@-]{0,127}@[0-9a-f]{64}\.service", unit),
                "invalid guarded runner unit")
        write_atomic(self.job_network_directory() / JOB_NETWORK_RUNNER_UNIT_FILE,
                     (unit + "\n").encode())

    def guard_job_network_start(self):
        """Fail closed unless the installed foundation, policy and units are live."""
        activation = self.guarded_activation()
        if activation is None:
            return None
        require(os.geteuid() == 0, "guarded job network startup requires root")
        for unit in (activation["guard_unit"], activation["policy_unit"], activation["runner_unit"]):
            require(self.unit_state(unit)["LoadState"] == "loaded",
                    "guarded job network unit is not installed")
        for unit in (activation["guard_unit"], activation["policy_unit"]):
            require(self.unit_state(unit)["ActiveState"] == "active",
                    "guarded foundation or allow policy is not active")
        for family in ("inet", "bridge"):
            data = json.loads(run(["/usr/sbin/nft", "-j", "list", "table", family, "piping_job"]))
            tables = [item["table"] for item in data.get("nftables", []) if "table" in item]
            require(len(tables) == 1 and tables[0].get("comment") == activation["foundation_comment"],
                    "guarded job network foundation is missing or foreign")
        identity = self.job_network_identity(activation)
        actual = json.loads(run(["docker", "network", "inspect", identity["Name"]]))[0]
        require({key: actual[key] for key in identity} == identity,
                "guarded job network identity drift")
        require(actual["Name"] == activation["network"]
                and actual["Options"].get("com.docker.network.bridge.name") == activation["bridge"],
                "guarded job network identity mismatch")
        require(self.unit_state(activation["runner_unit"])["LoadState"] == "loaded",
                "guarded runner template is not installed")
        return activation

    def inspect_runner_identity(self, identifier):
        inspected = json.loads(run(["docker", "inspect", identifier]))[0]
        labels = inspected["Config"].get("Labels") or {}
        require(inspected["Id"] == identifier
                and inspected["Name"] == "/" + self.args.project_name + "-ansible-runner-1"
                and labels.get("com.docker.compose.project") == self.args.project_name
                and labels.get("com.docker.compose.service") == "ansible-runner"
                and labels.get("com.docker.compose.project.working_dir") == str(self.root),
                "foreign guarded runner container")
        return inspected

    def validate_guarded_runner(self, identifier, activation, *, running=None, image=None):
        inspected = self.inspect_runner_identity(identifier)
        require(inspected["HostConfig"]["RestartPolicy"]["Name"] == "no",
                "guarded runner must persist restart=no")
        values = [value.partition("=")[2] for value in inspected["Config"].get("Env") or []
                  if value.startswith("PIPING_SANDBOX_DOCKER_NETWORK=")]
        require(values == [activation["network"]], "guarded runner job network mismatch")
        if image is not None:
            require(inspected["Image"] == image, "guarded runner image changed")
        if running is not None:
            require(inspected["State"]["Running"] is running, "guarded runner running state changed")
        return inspected

    def runner_identifier(self):
        identifiers = self.writer_containers("ansible-runner")
        require(len(identifiers) <= 1, "ambiguous installed runner identity")
        return identifiers[0] if identifiers else None

    def guarded_runner_stop(self):
        activation = self.guarded_activation()
        if activation is None:
            return
        unit = self.recorded_runner_unit()
        if unit:
            self.stop_runner_unit(unit, disable=False, check_container=True)
            return
        identifier = self.runner_identifier()
        if identifier is not None:
            inspected = json.loads(run(["docker", "inspect", identifier]))[0]
            require(inspected["State"]["Running"] is False,
                    "runner is active without a recorded guarded unit")

    def rollback_activation(self, activation):
        """Remove a failed first activation only after proving the runner is stopped."""
        cleaned = False
        try:
            unit = self.recorded_runner_unit()
            if unit:
                self.stop_runner_unit(unit, disable=True, check_container=True)
            identifier = self.runner_identifier()
            if identifier is not None:
                inspected = json.loads(run(["docker", "inspect", identifier]))[0]
                if inspected["State"]["Running"]:
                    run(["docker", "stop", "--time", "20", identifier], timeout=60)
                require(json.loads(run(["docker", "inspect", identifier]))[0]["State"]["Running"] is False,
                        "guarded runner remained active during activation rollback")
            directory = self.job_network_directory()
            for name in (JOB_NETWORK_ACTIVATION_FILE, JOB_NETWORK_OVERLAY):
                path = directory / name
                if os.path.lexists(path):
                    require(path.is_file() and not path.is_symlink() and path.stat().st_uid == 0,
                            "unsafe activation rollback path")
                    path.unlink()
            sync_directory(directory)
            self._guarded_activation = None
            cleaned = True
        finally:
            if not cleaned:
                # Keep the valid activation authority when cleanup cannot prove
                # the runner is stopped. A later start must fail closed, not
                # silently become an ordinary network-disabled start.
                self._guarded_activation = activation

    def guarded_runner_start(self, *, recreate, startup=None, expected_image=None):
        """Recreate stopped, record the immutable ID, then start only through the gate."""
        activation = self.guard_job_network_start()
        require(activation is not None, "guarded job network is not activated")
        previous = self.recorded_runner_unit()
        if previous:
            self.stop_runner_unit(previous, disable=True, check_container=True)
        if recreate:
            command = self.compose(startup=startup) + [
                "up", "--no-start", "--no-deps", "--force-recreate", "--pull", "never",
                "--no-build", "ansible-runner",
            ]
            run(command, timeout=210)
        identifier = self.runner_identifier()
        require(identifier is not None, "guarded runner container is missing")
        inspected = self.validate_guarded_runner(identifier, activation, running=None, image=expected_image)
        if not recreate and inspected["State"]["Running"]:
            # An explicit start must not adopt a process that did not come through
            # the gate. Stop it, then start the recorded immutable-ID unit.
            run(["docker", "stop", "--time", "20", identifier], timeout=60)
            inspected = self.validate_guarded_runner(identifier, activation, running=None, image=expected_image)
        require(inspected["State"]["Running"] is False, "guarded runner was not recreated stopped")
        unit = runner_instance_unit(activation["runner_unit"], identifier)
        self.write_recorded_runner_unit(unit)
        run(["systemctl", "enable", unit], timeout=60)
        run(["systemctl", "start", unit], timeout=90)
        state = self.unit_state(unit)
        require(state["LoadState"] == "loaded" and state["ActiveState"] == "active",
                "guarded runner unit did not start")
        self.validate_guarded_runner(identifier, activation, running=True, image=expected_image)
        return identifier

    def ensure_guarded_runner(self):
        activation = self.guard_job_network_start()
        require(activation is not None, "guarded job network is not activated")
        identifier = self.runner_identifier()
        if identifier is None:
            return self.guarded_runner_start(recreate=True)
        unit = self.recorded_runner_unit()
        if unit is not None:
            state = self.unit_state(unit)
            if state["LoadState"] == "loaded" and state["ActiveState"] == "active":
                inspected = self.validate_guarded_runner(identifier, activation, running=None)
                if inspected["State"]["Running"]:
                    return identifier
        return self.guarded_runner_start(recreate=False)

    def start_runtime_services(self, services):
        """Restore non-writer services; controlled_start owns writer lifecycle."""
        writers = self.controlled_writers()
        require(set(writers + ["frontend"]) <= self.running(), "controlled startup is incomplete")
        self.guard_writer_ids()
        if self.guarded_activation() is not None:
            self.guard_job_network_start()
        # Base Compose omits the authenticated startup labels. Applying it to
        # writers would replace their containers and make matched recovery fail.
        others = [service for service in services if service not in {*writers, "frontend"}]
        if others:
            run(self.compose() + ["up", "-d", "--no-deps", "--no-build", "--pull", "never",
                                  "--wait", "--wait-timeout", "180", *others], timeout=210)
        self.guard_writer_ids()

    def activate_job_network(self, network, bridge):
        """Adopt the installed native foundation and bind the guarded runner lifecycle."""
        require(os.geteuid() == 0, "job network activation requires root")
        directory = self.job_network_directory()
        require(directory.is_dir() and not directory.is_symlink()
                and stat.S_IMODE(directory.stat().st_mode) == 0o700
                and directory.stat().st_uid == 0, "installed job network directory is missing or unsafe")
        base = self.compose_path()
        require(base.is_file() and not base.is_symlink(), "installed Compose input is missing")
        require(os.path.lexists(self.root / ENV_FILE), "installed environment is missing")
        self.plan["active_overlay"] = self.active_overlay_input(allow_incomplete=True)
        ordinary_model = json.loads(run(self.compose(guarded=False) + ["config", "--format", "json"]))
        require("ansible-runner" in ordinary_model.get("services", {}),
                "installed image assignment does not resolve the runner service")
        requested = {
            "schema": JOB_NETWORK_ACTIVATION,
            "root": str(self.root),
            "project_name": self.args.project_name,
            "network": network,
            "bridge": bridge,
            "compose_file": base.name,
            "guard_unit": "piping-job-guard.service",
            "policy_unit": "piping-job-policy.service",
            "runner_unit": "piping-job-runner@.service",
            "foundation_comment": "piping-job-network:" + str(self.root) + ":v1",
            "restart": "no",
        }
        require(re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}", network)
                and re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,14}", bridge),
                "invalid job network activation arguments")
        existing = self.guarded_activation(allow_incomplete=True)
        if existing is not None:
            require(existing == requested, "job network authority changed after activation")
        else:
            for name in ("guard.nft", "deny.nft", "policy.nft", "site.nft", "endpoints.nft"):
                path = directory / name
                require(path.is_file() and not path.is_symlink()
                        and stat.S_IMODE(path.stat().st_mode) & 0o022 == 0
                        and path.stat().st_uid == 0, "installed job network input is unsafe: " + name)
            require(not os.path.lexists(directory / JOB_NETWORK_ACTIVATION_FILE),
                    "job network activation state is incomplete")
            first = self.runner_identifier()
            if first is not None:
                require(self.inspect_runner_identity(first)["State"]["Running"] is False,
                        "stop the runner before first job network activation")
        # Start the native foundation now; failure aborts before any activation record.
        run(["systemctl", "enable", requested["guard_unit"], requested["policy_unit"]], timeout=60)
        run(["systemctl", "start", requested["guard_unit"]], timeout=60)
        run(["systemctl", "start", requested["policy_unit"]], timeout=60)
        identity = self.job_network_identity(requested)
        actual = json.loads(run(["docker", "network", "inspect", identity["Name"]]))[0]
        require({key: actual[key] for key in identity} == identity,
                "installed job network does not match its recorded identity")
        for family in ("inet", "bridge"):
            data = json.loads(run(["/usr/sbin/nft", "-j", "list", "table", family, "piping_job"]))
            tables = [item["table"] for item in data.get("nftables", []) if "table" in item]
            require(len(tables) == 1 and tables[0].get("comment") == requested["foundation_comment"],
                    "installed job network foundation is missing or foreign")
        created = existing is None
        marker_path = directory / JOB_NETWORK_DEACTIVATION_FILE
        marker_bytes = None
        if created and os.path.lexists(marker_path):
            require(marker_path.is_file() and not marker_path.is_symlink()
                    and marker_path.stat().st_uid == 0, "unsafe job network deactivation state")
            marker_bytes = read(marker_path)
        try:
            if created:
                write_atomic(directory / JOB_NETWORK_OVERLAY, self.guarded_overlay_bytes(requested))
                write_atomic(directory / JOB_NETWORK_ACTIVATION_FILE,
                             json.dumps(requested, sort_keys=True, separators=(",", ":")).encode())
                marker = directory / JOB_NETWORK_DEACTIVATION_FILE
                if os.path.lexists(marker):
                    require(marker.is_file() and not marker.is_symlink() and marker.stat().st_uid == 0,
                            "unsafe job network deactivation state")
                    marker.unlink()
                    sync_directory(directory)
                self._guarded_activation = requested
            activation = self.guard_job_network_start()
            require(activation == requested, "job network activation did not persist")
            current = self.runner_identifier()
            if current is None:
                raise DeployError("guarded runner container is missing")
            inspected = self.inspect_runner_identity(current)
            if inspected["State"]["Running"]:
                require(existing is not None, "stop the runner before first job network activation")
                self.validate_guarded_runner(current, activation, running=True)
                unit = self.recorded_runner_unit()
                state = self.unit_state(unit) if unit else {"LoadState": "not-found"}
                require(unit is not None and state["LoadState"] == "loaded"
                        and state["ActiveState"] == "active",
                        "runner is active outside the guarded startup owner")
                return {"activation": file_sha(directory / JOB_NETWORK_ACTIVATION_FILE),
                        "runner": current, "unit": unit}
            return {"activation": file_sha(directory / JOB_NETWORK_ACTIVATION_FILE),
                    "runner": self.guarded_runner_start(recreate=True, expected_image=inspected["Image"]),
                    "unit": self.recorded_runner_unit()}
        except BaseException:
            if created:
                self.rollback_activation(requested)
                if (marker_bytes is not None
                        and not os.path.lexists(directory / JOB_NETWORK_ACTIVATION_FILE)
                        and not os.path.lexists(directory / JOB_NETWORK_OVERLAY)):
                    write_atomic(marker_path, marker_bytes)
                    self._guarded_activation = None
            raise

    def deactivate_job_network(self):
        """Return the runner to the network-disabled lifecycle without touching the native foundation."""
        require(os.geteuid() == 0, "job network deactivation requires root")
        activation = self.guarded_activation()
        require(activation is not None, "job network is not activated")
        self.guard_job_network_start()
        self.plan["active_overlay"] = self.active_overlay_input()
        resolved = json.loads(run(self.compose(guarded=False) + ["config", "--format", "json"]))
        ordinary = resolved["services"]["ansible-runner"].get("environment", {}).get(
            "PIPING_SANDBOX_DOCKER_NETWORK", "")
        require(ordinary == "", "deactivate requires an empty PIPING_ANSIBLE_JOB_NETWORK")
        unit = self.recorded_runner_unit()
        if unit:
            self.stop_runner_unit(unit, disable=True, check_container=True)
        else:
            identifier = self.runner_identifier()
            if identifier is not None:
                require(json.loads(run(["docker", "inspect", identifier]))[0]["State"]["Running"] is False,
                        "guarded runner did not stop")
        identifier = self.runner_identifier()
        require(identifier is not None, "guarded runner container is missing")
        inspected = self.validate_guarded_runner(identifier, activation, running=None)
        require(inspected["State"]["Running"] is False, "guarded runner did not stop")
        directory = self.job_network_directory()
        moved = []
        try:
            command = self.compose(guarded=False) + ["up", "--no-start", "--no-deps", "--force-recreate",
                                                     "--pull", "never", "--no-build", "ansible-runner"]
            run(command, timeout=210)
            identifier = self.runner_identifier()
            require(identifier is not None, "ordinary runner container is missing")
            inspected = json.loads(run(["docker", "inspect", identifier]))[0]
            values = [value.partition("=")[2] for value in inspected["Config"].get("Env") or []
                      if value.startswith("PIPING_SANDBOX_DOCKER_NETWORK=")]
            require(values == [""] and inspected["State"]["Running"] is False,
                    "ordinary runner did not return to the network-disabled stopped state")
            run(self.compose(guarded=False) + ["start", "ansible-runner"], timeout=90)
            inspected = json.loads(run(["docker", "inspect", identifier]))[0]
            require(inspected["State"]["Running"] is True,
                    "ordinary runner did not start")
            # Commit the ownership change only after the ordinary runner is verified.
            for name in (JOB_NETWORK_ACTIVATION_FILE, JOB_NETWORK_OVERLAY):
                source = directory / name
                require(source.is_file() and not source.is_symlink() and source.stat().st_uid == 0,
                        "unsafe job network activation state")
                temporary = directory / ("." + name + ".deactivating-" + uuid.uuid4().hex)
                os.replace(source, temporary)
                moved.append((source, temporary))
            sync_directory(directory)
            for _, temporary in moved:
                temporary.unlink()
            write_atomic(directory / JOB_NETWORK_DEACTIVATION_FILE,
                         json.dumps({"schema": "piping.job-network-deactivated/v1",
                                     "root": str(self.root),
                                     "project_name": self.args.project_name},
                                    sort_keys=True, separators=(",", ":")).encode())
            sync_directory(directory)
            self._guarded_activation = None
        except BaseException:
            # Never leave an ordinary runner active while ownership is ambiguous.
            with contextlib.suppress(OSError, ValueError, subprocess.SubprocessError):
                run(self.compose(guarded=False) + ["stop", "--timeout", "30", "ansible-runner"], timeout=60)
            with contextlib.suppress(OSError, ValueError, subprocess.SubprocessError):
                identifier = self.runner_identifier()
                if identifier is not None:
                    run(["docker", "stop", "--time", "20", identifier], timeout=60)
            for source, temporary in reversed(moved):
                if os.path.lexists(temporary):
                    with contextlib.suppress(OSError):
                        os.replace(temporary, source)
            marker = directory / JOB_NETWORK_DEACTIVATION_FILE
            if os.path.lexists(marker):
                with contextlib.suppress(OSError):
                    marker.unlink()
            try:
                write_atomic(directory / JOB_NETWORK_OVERLAY, self.guarded_overlay_bytes(activation))
                write_atomic(directory / JOB_NETWORK_ACTIVATION_FILE,
                             json.dumps(activation, sort_keys=True, separators=(",", ":")).encode())
                self._guarded_activation = activation
            except (OSError, DeployError):
                self._guarded_activation = None
            with contextlib.suppress(OSError):
                for _, temporary in moved:
                    if os.path.lexists(temporary):
                        temporary.unlink()
                sync_directory(directory)
            raise
        return {"runner": identifier}

    @contextlib.contextmanager
    def lock(self):
        # Serialize every stage for this root, including distinct stage directories.
        path = self.root / ".deploy-release.lock"
        descriptor = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        try:
            def guard():
                opened = os.fstat(descriptor)
                named = path.lstat()
                require(
                    all(stat.S_ISREG(item.st_mode) and item.st_nlink == 1
                        and stat.S_IMODE(item.st_mode) == 0o600 for item in (opened, named))
                    and (opened.st_dev, opened.st_ino) == (named.st_dev, named.st_ino),
                    "unsafe deployment lock",
                )

            guard()
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise ConcurrentOperation("Another installation/update is active") from error
            guard()
            yield
        finally:
            os.close(descriptor)

    @staticmethod
    def startup_ownership(model):
        require(isinstance(model, dict) and set(model) == {"services"} and isinstance(model["services"], dict)
                and {"backend", "ansible-runner"} <= set(model["services"]) <= set(WRITERS),
                "unsupported startup overlay services")
        nonces = set()
        for definition in model["services"].values():
            require(isinstance(definition, dict) and set(definition) == {"labels"}
                    and isinstance(definition["labels"], dict) and set(definition["labels"]) == {"piping.preparation-start"},
                    "overlay changes more than startup ownership")
            nonce = definition["labels"]["piping.preparation-start"]
            require(isinstance(nonce, str) and re.fullmatch(r"[0-9a-f]{32}", nonce), "invalid startup ownership nonce")
            nonces.add(nonce)
        require(len(nonces) == 1, "ambiguous startup ownership nonce")
        return next(iter(nonces))

    def active_overlay_input(self, *, allow_incomplete=False):
        base = str(self.compose_path())
        guarded = (str(self.guarded_compose_path(allow_incomplete=allow_incomplete))
                   if self.guarded_activation(allow_incomplete=allow_incomplete) else None)
        observed = {}
        overlays = set()
        for role in WRITERS:
            ids = self.writer_containers(role)
            require(len(ids) <= 1, "ambiguous writer while resolving active Compose input")
            if not ids:
                continue
            labels = json.loads(run(["docker", "inspect", ids[0]]))[0]["Config"].get("Labels", {})
            files = labels.get("com.docker.compose.project.config_files", "").split(",")
            require(files and files[0] == base and len(files) == len(set(files)),
                    "unsupported installed Compose overlay chain")
            extras = files[1:]
            require(len(extras) <= (1 + (1 if guarded else 0)), "unsupported installed Compose overlay chain")
            startup = [item for item in extras if guarded is None or item != guarded]
            require(all(Path(item).name == "startup.compose.json" for item in startup)
                    and len(startup) <= 1, "unsupported installed Compose overlay chain")
            if startup:
                overlays.add(startup[0])
            observed[role] = ([files[0], *startup], labels.get("piping.preparation-start"))
        require(len(overlays) <= 1, "writers disagree on the active startup overlay")
        if not overlays:
            return None
        path = Path(next(iter(overlays)))
        require(path.name == "startup.compose.json" and path.parent.parent == self.root / "backups"
                and re.fullmatch(r"release-[0-9]{8}T[0-9]{6}Z", path.parent.name), "unsupported active startup overlay path")
        model = read_json(path)
        nonce = self.startup_ownership(model)
        require(all(observed.get(role) == ([base, str(path)], nonce) for role in model["services"]),
                "active startup label differs from its installed writer")
        return {"path": str(path), "sha256": file_sha(path)}

    def guard_active_overlay(self):
        overlay = self.plan.get("active_overlay")
        if overlay:
            require(set(overlay) == {"path", "sha256"}
                    and file_sha(Path(overlay["path"])) == overlay["sha256"]
                    == file_sha(self.stage / "active-overlay.original"), "active startup overlay changed")

    def guard_runner_roots(self):
        values = self.plan.get("runner_deployment")
        if not values:
            return
        endpoint = environment().get("DOCKER_HOST") or run([
            "docker", "context", "inspect", "--format", "{{.Endpoints.docker.Host}}"]).decode().strip()
        require(endpoint in ("unix:///var/run/docker.sock", "unix:///run/docker.sock"),
                "sandbox adoption requires the local Docker socket")
        metadata = Path("/var/run/docker.sock").stat()
        require(stat.S_ISSOCK(metadata.st_mode) and str(metadata.st_gid) == values["PIPING_DOCKER_GID"],
                "sandbox Docker socket identity changed")
        for key, name in (("PIPING_ANSIBLE_STATE_ROOT", "ansible-state"), ("PIPING_ANSIBLE_JOB_ROOT", "ansible-jobs")):
            path = Path(values[key])
            require(path == self.root / name, "unsafe runner private root")
            checked_path(path, directory=True)
            metadata = path.stat()
            require(metadata.st_uid == metadata.st_gid == 10001 and stat.S_IMODE(metadata.st_mode) == 0o700,
                    "runner private root permissions changed")

    def compose(self, images=None, *, startup=None, original=False, guarded=None):
        require(not (original and images is not None), "original and candidate Compose inputs cannot be combined")
        image_file = self.plan.get("image_file") or self.installed_image_file()
        candidate = self.plan.get("candidate_config", {}) if images is not None else {}
        compose_name = self.compose_path().name
        env = self.stage / candidate[ENV_FILE] if ENV_FILE in candidate else self.root / ENV_FILE
        compose_file = self.stage / candidate[compose_name] if compose_name in candidate else self.compose_path()
        assignments = self.root / image_file
        if original:
            env, compose_file = self.stage / ".env.original", self.stage / (compose_name + ".original")
            assignments = self.stage / (image_file + ".original")
        env_files = [str(images or env)] if image_file == ".env" else [
            str(env), str(images or assignments)
        ]
        overlays = []
        active = startup or (Path(self.plan["active_overlay"]["path"]) if self.plan.get("active_overlay") else None)
        if active:
            overlays.append(active)
        include_guarded = self.guarded_activation() is not None if guarded is None else guarded
        if include_guarded:
            path = self.job_network_directory() / JOB_NETWORK_OVERLAY
            require(path.is_file(), "guarded Compose overlay is missing")
            overlays.append(path)
        return [
            "docker",
            "compose",
            "--project-name",
            self.args.project_name,
            "--project-directory",
            str(self.root),
            *(part for name in env_files for part in ("--env-file", name)),
            "-f",
            str(compose_file),
            *(part for overlay in overlays for part in ("-f", str(overlay))),
            "--profile",
            "*",
        ]

    def runner_candidate_inputs(self, directory, originals):
        compose_name = self.compose_path().name
        required = {ENV_FILE, compose_name}
        checked_path(directory, directory=True)
        require(stat.S_IMODE(directory.stat().st_mode) == 0o700
                and set(path.name for path in directory.iterdir()) == required,
                "runner candidate must be a private directory with exactly .env and the installed Compose input")
        candidates = {name: read(directory / name) for name in required}
        require(all(stat.S_IMODE((directory / name).stat().st_mode) == 0o600 for name in required),
                "runner candidate files must be private")
        keys = {"PIPING_DOCKER_GID", "PIPING_ANSIBLE_STATE_ROOT", "PIPING_ANSIBLE_JOB_ROOT", "PIPING_ANSIBLE_JOB_NETWORK"}
        def split_environment(content):
            values, retained = {}, []
            for line in content.decode().splitlines(keepends=True):
                name, separator, value = line.rstrip("\n").partition("=")
                if name in keys and separator:
                    require(name not in values and not any(c in value for c in "\r\x00\"'$`\\"),
                            "runner deployment keys require unique literal assignments")
                    values[name] = value
                else:
                    retained.append(line)
            return values, retained
        values, retained = split_environment(candidates[".env"])
        _, original_retained = split_environment(originals[".env"])
        require(set(values) == keys and retained == original_retained,
                "runner candidate changed environment beyond the four deployment keys")
        activation = self.guarded_activation()
        expected_network = activation["network"] if activation else ""
        require(values["PIPING_ANSIBLE_JOB_NETWORK"] == expected_network,
                "runner candidate network authority differs from the installed activation")
        socket_path = Path("/var/run/docker.sock")
        socket_stat = socket_path.stat()
        require(stat.S_ISSOCK(socket_stat.st_mode) and values["PIPING_DOCKER_GID"] == str(socket_stat.st_gid),
                "runner socket GID differs from the local Docker socket")
        for key, name in (("PIPING_ANSIBLE_STATE_ROOT", "ansible-state"), ("PIPING_ANSIBLE_JOB_ROOT", "ansible-jobs")):
            path = Path(values[key])
            require(path == self.root / name, "runner private root must be its dedicated installation directory")
            if not os.path.lexists(path):
                checked_path(path, missing=True)
                prefix = [] if os.geteuid() == 0 else ["sudo", "-n"]
                run([*prefix, "mkdir", "--mode=0700", "--", str(path)])
                run([*prefix, "chown", "--no-dereference", "10001:10001", "--", str(path)])
                sync_directory(self.root)
            checked_path(path, directory=True)
            metadata = path.stat()
            require(metadata.st_uid == metadata.st_gid == 10001 and stat.S_IMODE(metadata.st_mode) == 0o700,
                    "runner private root has unsafe ownership or permissions")
        self.plan["runner_deployment"] = values
        return candidates

    def guard_runner_candidate(self, before, after):
        values = self.plan["runner_deployment"]
        roots = [Path(values[key]) for key in ("PIPING_ANSIBLE_STATE_ROOT", "PIPING_ANSIBLE_JOB_ROOT")]
        for service in before["services"].values():
            for mount in service.get("volumes", []):
                if mount.get("type") == "bind":
                    path = Path(mount["source"])
                    require(not any(path == root or path in root.parents or root in path.parents for root in roots),
                            "sandbox root overlaps an existing application mount")
        prior = before["services"]["ansible-runner"]
        actual = after["services"]["ansible-runner"]
        health = prior.get("healthcheck")
        require(isinstance(health, dict) and health.get("disable", False) is False
                and health.get("test") in (RUNNER_HEALTH_COMMAND, RETIRED_RUNNER_HEALTH_COMMAND),
                "runner healthcheck is disabled, absent or unknown")
        mounts = {"/var/run/docker.sock": ("/var/run/docker.sock", True),
                  "/var/lib/piping-runner": (values["PIPING_ANSIBLE_STATE_ROOT"], False),
                  "/var/lib/piping-jobs": (values["PIPING_ANSIBLE_JOB_ROOT"], False)}
        expected = json.loads(json.dumps(prior))
        expected["healthcheck"]["test"] = RUNNER_HEALTH_COMMAND
        expected.update({"user": "10001:10001", "read_only": True,
                         "group_add": list(dict.fromkeys([*prior.get("group_add", []), values["PIPING_DOCKER_GID"]])),
                         "cap_drop": list(dict.fromkeys([*prior.get("cap_drop", []), "ALL"])),
                         "security_opt": list(dict.fromkeys([*prior.get("security_opt", []), "no-new-privileges:true"]))})
        replaced_tmpfs = {"/tmp", "/home/runner", *mounts}
        expected["tmpfs"] = [item for item in prior.get("tmpfs", []) if item.split(":", 1)[0] not in replaced_tmpfs]
        expected["tmpfs"] += ["/home/runner:exec,mode=0770,uid=10001,gid=10001,size=128m", "/tmp:exec,mode=1777,size=256m"]
        activation = self.guarded_activation()
        expected["environment"] = {**prior.get("environment", {}),
            "PIPING_SANDBOX_RUNTIME": "docker", "PIPING_SANDBOX_INSTALLATION_ID": self.args.project_name,
            "PIPING_SANDBOX_WORKSPACE_ROOT": "/var/lib/piping-jobs",
            "PIPING_SANDBOX_HOST_WORKSPACE_ROOT": values["PIPING_ANSIBLE_JOB_ROOT"],
            "PIPING_SANDBOX_RUNTIME_DIR": "/var/lib/piping-runner/runtime",
            "PIPING_SANDBOX_DOCKER_NETWORK": activation["network"] if activation else ""}
        require(not any(mount["target"] in mounts for mount in prior.get("volumes", [])),
                "existing runner mount conflicts with sandbox adoption")
        preserved, adopted = [], []
        for mount in actual.get("volumes", []):
            target = mount["target"]
            if target not in mounts:
                preserved.append(mount)
                continue
            source, readonly = mounts[target]
            require(set(mount) <= {"type", "source", "target", "read_only", "bind"}
                    and mount.get("type") == "bind" and mount.get("source") == source
                    and mount.get("read_only", False) is readonly
                    and set(mount.get("bind", {})) <= {"create_host_path"}
                    and type(mount.get("bind", {}).get("create_host_path", False)) is bool,
                    "runner sandbox mount exceeds authorized shape")
            adopted.append(target)
        require(preserved == prior.get("volumes", []) and sorted(adopted) == sorted(mounts),
                "runner candidate changed existing mounts or omitted a private sandbox mount")
        expected["volumes"] = actual["volumes"]
        require(expected == actual, "runner candidate changed settings beyond the authorized sandbox stanza")
        normalized = json.loads(json.dumps(after))
        normalized["services"]["ansible-runner"] = prior
        require(normalized == before, "runner candidate changed a non-runner service, secret, mount or deployment setting")

    def phase(self):
        checked_path(self.stage, directory=True)
        checked_path(self.stage / "journal", directory=True)
        for directory in (self.root / "backups", self.stage, self.stage / "journal"):
            checked_path(directory, directory=True)
            require(
                stat.S_IMODE(directory.stat().st_mode) == 0o700,
                "stage directories must be private",
            )
        names = sorted(path.name for path in (self.stage / "journal").iterdir())
        require(names and len(names) <= len(PHASES) + len(PREPARATION_PHASES) + 1, "invalid phase journal")
        previous = None
        for index, name in enumerate(names):
            phase = name[3:]
            require(
                name == f"{index:02d}-{phase}" and read(self.stage / "journal" / name) == b"\n"
                and (phase == "staging" if previous is None else phase in PHASE_EDGES.get(previous, ())),
                "invalid phase journal",
            )
            require(
                stat.S_IMODE((self.stage / "journal" / name).stat().st_mode) == 0o600,
                "phase journal must be private",
            )
            previous = phase
        return previous

    def advance(self, phase):
        index = len(tuple((self.stage / "journal").iterdir()))
        require(phase == "staging" if not index else phase in PHASE_EDGES.get(self.phase(), ()),
                "phase cannot be replayed")
        write(self.stage / "journal" / f"{index:02d}-{phase}", b"\n")

    def historical_startup_input(self):
        """Authenticate retained ownership provenance, never execution authority."""
        kinds = {kind + "-started": kind for kind in ("original-r", "backup", "cutover-r")}
        kind = kinds.get(self.phase())
        require(kind is not None, "historical input requires completed recovery startup")
        if not read_json(self.stage / "plan.json").get("active_overlay"):
            return None
        key = self.history_verification_key
        require(key is not None and file_sha(key) == file_sha(self.stage / "verification-key.pem"),
                "historical recovery requires the trusted signer key")
        self.verifier_key = key
        self.authenticate_stage_data()
        # The signed immutable request/manifest, not a mutable request alias or
        # the local completion journal, authenticates the retained input bytes.
        manifest = self.snapshot_manifest_data(kind, self.plan["helper_sha256"], authenticated=True)
        recovery = read_json(self.stage / (kind + "-recovery.json"))
        require(set(recovery) == {"source", "manifest_sha256", "database", "quarantine", "writers_stopped"}
                and recovery["source"] == kind
                and recovery["manifest_sha256"] == file_sha(self.stage / (kind + "-manifest.json"))
                and recovery["database"] == "piping" and recovery["writers_stopped"] is True
                and isinstance(recovery["quarantine"], str)
                and re.fullmatch(r"piping_quarantine_[0-9a-f]{32}", recovery["quarantine"]),
                "historical recovery completion binding mismatch")
        overlay = self.plan["active_overlay"]
        require(isinstance(overlay, dict) and set(overlay) == {"path", "sha256"}, "invalid historical input record")
        path = Path(overlay["path"])
        require(path.name == "startup.compose.json" and path.parent.parent == self.root / "backups"
                and re.fullmatch(r"release-[0-9]{8}T[0-9]{6}Z", path.parent.name)
                and path.parent.name < self.stage.name,
                "historical startup input is not an older owned path")
        retained = self.stage / "active-overlay.original"
        require(overlay["sha256"] == file_sha(retained) == manifest["files"]["active-overlay.original"],
                "historical startup input differs from authenticated retained bytes")
        self.startup_ownership(read_json(retained))
        return overlay

    def guard_startup_input_directory(self, stage, overlay):
        require(overlay["path"] == str(stage / "startup.compose.json"), "startup input path differs from provenance")
        checked_path(stage, directory=True)
        require(stat.S_IMODE(stage.stat().st_mode) == 0o700
                and {path.name for path in stage.iterdir()} == {"startup.compose.json"},
                "startup input contains unclassified recovery files")
        path = stage / "startup.compose.json"
        require(stat.S_IMODE(path.stat().st_mode) == 0o600 and file_sha(path) == overlay["sha256"],
                "startup input bytes or permissions changed")
        self.startup_ownership(read_json(path))

    def guard_recovery(self):
        backups = self.root / "backups"
        if not os.path.lexists(backups):
            return
        checked_path(backups, directory=True)
        historical = {}
        for stage in sorted(backups.iterdir(), reverse=True):
            if not stage.name.startswith("release-"):
                continue
            owner = Deployment(argparse.Namespace(
                root=self.root, stage=stage, project_name=self.args.project_name,
                compose_file=self.compose_file, verification_key=self.history_verification_key,
            ))
            if not os.path.lexists(stage / "journal"):
                active = self.active_overlay_input()
                overlay = active if active and active["path"] == str(stage / "startup.compose.json") else historical.get(str(stage / "startup.compose.json"))
                if overlay:
                    self.guard_startup_input_directory(stage, overlay)
                    continue
            phase = owner.phase()
            if phase == ABORTED_OLD_AVAILABILITY:
                owner.validate_abort_history()
                return
            if phase not in ("staging", "staged", "backup-starting", "healthy", "original-r-started", "backup-started", "cutover-r-started"):
                raise RecoveryRequired("Unresolved update journal; inspect backups and recover explicitly before starting or updating")
            if phase in ("original-r-started", "backup-started", "cutover-r-started"):
                overlay = owner.historical_startup_input()
                if overlay:
                    require(overlay["path"] not in historical or historical[overlay["path"]] == overlay,
                            "conflicting historical startup provenance")
                    historical[overlay["path"]] = overlay

    def refresh_edge_upstreams(self):
        # Compose ps includes retained services absent from the current file.
        # Address the unique owned container, not a declaration or mutable name.
        identifiers = run([
            "docker", "ps", "-a", "--no-trunc",
            "--filter", "label=com.docker.compose.project=" + self.args.project_name,
            "--filter", "label=com.docker.compose.service=nginx", "--format", "{{.ID}}",
        ]).decode().splitlines()
        if not identifiers:
            require("nginx" not in self.declared_services(), "declared nginx container is missing")
            return
        require(len(identifiers) == 1 and re.fullmatch(r"[0-9a-f]{64}", identifiers[0]),
                "unique Compose-owned nginx container required")
        identifier = identifiers[0]
        inspected = json.loads(run(["docker", "inspect", identifier]))
        require(isinstance(inspected, list) and len(inspected) == 1 and isinstance(inspected[0], dict),
                "invalid edge container inspection")
        container = inspected[0]
        config, state = container.get("Config"), container.get("State")
        require(isinstance(config, dict) and isinstance(state, dict), "invalid edge container inspection")
        labels = config.get("Labels")
        require(container.get("Id") == identifier
                and isinstance(labels, dict)
                and labels.get("com.docker.compose.project") == self.args.project_name
                and labels.get("com.docker.compose.service") == "nginx"
                and labels.get("com.docker.compose.oneoff", "False") in ("False", "false")
                and state.get("Running") is True,
                "edge container identity or running state changed")
        # Reload re-resolves application DNS without replacing customer ingress.
        # A failed config check must leave the existing master configuration live.
        run(["docker", "exec", identifier, "nginx", "-t"])
        run(["docker", "exec", identifier, "nginx", "-s", "reload"])

    def verify_runtime_health(self):
        self.load("started-awaiting-health")
        try:
            self.verify_applied_runtime_health()
        except BaseException:
            if os.path.lexists(self.stage / "access-fence.json"):
                self.retain_access_fence()
            raise

    def verify_applied_runtime_health(self):
        """Accept a completed apply only after observing its actual candidate runtime."""
        self.guard_config(installed=True)
        self.inspect_images()
        self.runtime_health({role: self.image(role) for role in ROLES})
        self.guard_database(completed=True)
        self.advance("healthy")

    def runtime_health(self, images):
        for role in ROLES:
            require(self.service_image(role) == images[role], "runtime image selection differs")
            identifier = run(self.compose() + ["ps", "-aq", role]).decode().strip()
            state = json.loads(run(["docker", "inspect", identifier]))[0]["State"]
            require(state["Running"] and state.get("Health", {}).get("Status", "healthy") == "healthy",
                    "applied runtime is not healthy")
        run(self.compose() + ["exec", "-T", "backend", "curl", "--fail", "--silent",
                               "--max-time", "10", "http://127.0.0.1:7000/api/v1/ready"], timeout=20)
        self.refresh_edge_upstreams()

    def seal(self, name, files):
        write_json(self.stage / name, {file: file_sha(self.stage / file) for file in files})

    def verify_seal(self, name):
        sealed = read_json(self.stage / name)
        require(
            stat.S_IMODE((self.stage / name).stat().st_mode) == 0o600,
            "seal must be private",
        )
        require(isinstance(sealed, dict) and sealed, "invalid stage seal")
        for file, expected in sealed.items():
            require(
                isinstance(file, str) and Path(file).name == file and file_sha(self.stage / file) == expected,
                "stage seal mismatch",
            )
            require(
                stat.S_IMODE((self.stage / file).stat().st_mode) == 0o600,
                "sealed files must be private",
            )

    def authenticate_stage_data(self):
        self.verify_seal("stage-seal.json")
        self.plan = read_json(self.stage / "plan.json")
        require(socket.gethostname() == self.plan["target"], "executing host differs from staged target")
        require(
            self.plan["root"] == str(self.root) and self.plan["project_name"] == self.args.project_name,
            "stage ownership mismatch",
        )
        require(self.plan["image_file"] in (".env", ".images.env"), "invalid sealed image file")
        require(self.plan["compose_file"] == self.compose_file,
                "installed Compose filename differs from sealed stage")
        authenticate(self.stage / "package.json", self.stage / "package.sig", self.stage / "verification-key.pem")
        require(file_sha(self.stage / "package.json") == self.plan["package_sha256"]
                and file_sha(self.stage / "verification-key.pem") == self.plan["verification_key_sha256"],
                "stage authentication identity changed")
        package = read_json(self.stage / "package.json")
        require(package["schema"] == "piping.compose-delivery/v1"
                and package["helper_sha256"] == self.plan["helper_sha256"]
                and package["commit"] == self.plan["commit"] and package["pipeline_id"] == self.plan["pipeline_id"]
                and package["release_sha256"] == self.plan["release_sha256"]
                and package_binding(package) == self.plan["package_binding_sha256"]
                and package.get("preservation", {}).get("manifest_sha256") == self.plan["preservation_sha256"]
                 and package.get("preservation", {}).get("capture_sha256") == self.plan["check_capture_sha256"],
                  "preservation authority differs from signed package")
        if self.plan["preservation_sha256"]:
            require(file_sha(self.stage / "preservation.json") == self.plan["preservation_sha256"],
                    "retained preservation bytes changed")
        if self.plan["check_capture_sha256"]:
            require(file_sha(self.stage / "check-capture.json") == self.plan["check_capture_sha256"],
                    "retained CHECK capture bytes changed")

    def load(self, phase):
        self.guard_latest_stage()
        require(self.phase() == phase, "interrupted or completed phase cannot be replayed")
        self.authenticate_stage_data()
        require(self.plan["helper_sha256"] == helper_identity(), "helper changed after staging")
        require(self.plan.get("compose_file") == self.compose_file
                and self.plan["compose_file"] == self.compose_path().name,
                "installed Compose input changed after staging")
        activation = self.guarded_activation()
        activation_sha256 = (file_sha(self.job_network_directory() / JOB_NETWORK_ACTIVATION_FILE)
                             if activation else None)
        require(self.plan.get("job_network_activation_sha256") == activation_sha256,
                "guarded job network activation changed after staging")
        allowed_config = {} if not self.plan.get("candidate_config") else {
            ENV_FILE: "environment.next", self.compose_path().name: "compose.next"}
        require(self.plan.get("candidate_config") in ({}, allowed_config),
                "unsupported sealed configuration replacement")
        require(all(isinstance(self.plan.get(key), str) and re.fullmatch(r"[0-9a-f]{64}", self.plan[key])
                    for key in ("original_model_sha256", "candidate_model_sha256")),
                "sealed resolved Compose identities are missing")
        self.approval()
        require(getattr(self.args, "image_file", None) in (None, self.plan["image_file"]),
                "image assignment file differs from sealed stage")
        validate_contract(self.plan["contract"], self.plan["migrations"])

    def old_availability_evidence(self, result_path, signature, key):
        """Authenticate historical data without executing a retained helper."""
        self.authenticate_stage_data()
        self.verify_seal("backup-seal.json")
        require(file_sha(key) == self.plan["verification_key_sha256"]
                == file_sha(self.stage / "verification-key.pem"), "recovery authority key differs")
        authenticate(result_path, signature, key)
        request = read_json(self.stage / "backup-request.json")
        require(request["schema"] == "piping.compose-backup/v1" and request["preparation"] is None
                and request["helper_sha256"] == self.plan["helper_sha256"]
                and request["plan_sha256"] == file_sha(self.stage / "plan.json")
                and request["package_sha256"] == self.plan["package_sha256"]
                and request["target"] == self.plan["target"] and request["root"] == str(self.root)
                and request["stage"] == str(self.stage) and request["project_name"] == self.args.project_name
                and request["commit"] == self.plan["commit"] and request["pipeline_id"] == self.plan["pipeline_id"],
                "recovery requires an authenticated unapplied plain-C backup")
        require(not any(os.path.lexists(self.stage / name) for name in (
            "original-r-manifest.json", "consumed-receipt.json", "applied-restore-receipt.json",
            "applied-restore-receipt.sig")), "stage already has preparation or apply authority")
        for name, description in request["files"].items():
            require(Path(name).name == name and file_sha(self.stage / name) == description["sha256"]
                    and (self.stage / name).stat().st_size == description["size"], "retained backup bytes changed")
        require({"fingerprint.json", "access-fence.json", "original-pg_hba.conf", "runtime-inputs.json", "writers.json"}
                <= set(request["files"]), "recovery backup evidence is incomplete")
        raw = read(result_path)
        value = json.loads(raw, object_pairs_hook=reject_duplicate_json_keys)
        fence = read_json(self.stage / "access-fence.json")
        require(fence["plan_sha256"] == file_sha(self.stage / "plan.json")
                and fence["original_hba_sha256"] == file_sha(self.stage / "original-pg_hba.conf"),
                "recovery provider or original HBA binding changed")
        writers = value["state"]["writers"]
        require(set(writers) == set(self.controlled_writers())
                and all(writer["id"] == self.plan["writer_ids"][role] for role, writer in writers.items())
                and all(writers[role]["image"] == self.plan["target_backend_image"]
                        for role in ("backend", "proxy") if role in writers), "recovery old writer binding changed")
        baseline = read_json(self.stage / "fingerprint.json")
        require(baseline["history"] == self.plan["installed_history"], "backup installed history differs")
        state = {"target": self.plan["target"], "root": str(self.root), "stage": str(self.stage),
            "project": self.args.project_name, "helper_sha256": self.plan["helper_sha256"],
            "application_source": self.plan["commit"], "package_sha256": self.plan["package_sha256"],
            "plan_sha256": file_sha(self.stage / "plan.json"),
            "request_sha256": file_sha(self.stage / "backup-request.json"),
            "fingerprint_sha256": file_sha(self.stage / "fingerprint.json"),
            "fence_sha256": file_sha(self.stage / "access-fence.json"),
            "original_hba_sha256": file_sha(self.stage / "original-pg_hba.conf"),
            "provider_identity_sha256": sha(canonical(fence["postgres"])), "writers": writers,
            "original_config_hashes": self.plan["baseline"], "history_count": len(baseline["history"]),
            "full_baseline_equality": True, "daily_disabled": True, "no_apply": True}
        return manual_old_availability_result(raw, expected_state=state,
            expected_code_sha256=value["code_sha256"], expected_request_sha256=value["request_sha256"],
            compose_file=self.compose_file), fence

    def old_availability_observation(self, state, fence):
        self.guard_config()
        history = self.guard_database()
        self.observe_target_catalog()
        self.guard_runtime_inputs()
        require(self.postgres_identity() == fence["postgres"], "recovered PostgreSQL provider changed")
        require(self.hba_bytes(fence["postgres"]) == read(self.stage / "original-pg_hba.conf"),
                "recovered HBA is not the exact original")
        require(self.pinned_postgres_sql(fence["postgres"]["container"],
            "SELECT NOT EXISTS (SELECT FROM pg_hba_file_rules WHERE error IS NOT NULL);") == b"t",
            "recovered HBA rules are invalid")
        observed = {}
        for role, expected in state["writers"].items():
            require(self.writer_containers(role) == [expected["id"]], "recovered old writer was replaced")
            row = json.loads(run(["docker", "inspect", expected["id"]]))[0]
            labels = row["Config"].get("Labels", {})
            require(row["Id"] == expected["id"] and row["Image"] == expected["image"]
                    and labels.get("com.docker.compose.project") == self.args.project_name
                    and labels.get("com.docker.compose.service") == role
                    and row["State"]["Running"] is self.plan["writer_running"][role]
                    and sha(canonical({"config": row["Config"], "mounts": row["Mounts"]}))
                    == expected["config_mount_sha256"], "recovered old writer definition or state changed")
            observed[role] = {**expected, "running": row["State"]["Running"]}
        require(observed["backend"]["running"], "old backend was not restored")
        run(["docker", "exec", state["writers"]["backend"]["id"], "curl", "--fail", "--silent",
             "--max-time", "10", "http://127.0.0.1:7000/api/v1/ready"], timeout=20)
        return {"original_config_hashes": self.plan["baseline"], "history_sha256": sha(canonical(history)),
            "catalog_sha256": sha(canonical(self.plan["installed_target_catalog"])),
            "layout_sha256": sha(canonical(self.plan["installed_layout_slots"])),
            "check_bindings_sha256": sha(canonical(self.plan["installed_check_bindings"])),
            "provider_identity_sha256": sha(canonical(fence["postgres"])),
            "original_hba_sha256": state["original_hba_sha256"], "writers": observed,
            "daily_disabled": True, "hba_rules_valid": True, "old_backend_ready": True}

    def abort_bindings(self, result, *, acknowledging_helper, observation, observed_at):
        return {"retained_state": result["state"], "acknowledging_helper_sha256": acknowledging_helper,
            "operator_result_sha256": file_sha(self.stage / "abort-operator-result.json"),
            "operator_result_signature_sha256": file_sha(self.stage / "abort-operator-result.sig"),
            "verification_key_sha256": self.plan["verification_key_sha256"],
            "stage_seal_sha256": file_sha(self.stage / "stage-seal.json"),
            "backup_seal_sha256": file_sha(self.stage / "backup-seal.json"),
            "observed_old_state": observation, "observed_at": observed_at}

    def validate_abort_history(self):
        """Validate retained acknowledgment evidence, never later live state."""
        result, _ = self.old_availability_evidence(self.stage / "abort-operator-result.json",
            self.stage / "abort-operator-result.sig", self.stage / "verification-key.pem")
        raw = read(self.stage / "abort-acknowledgment.json")
        acknowledgment = json.loads(raw, object_pairs_hook=reject_duplicate_json_keys)
        observation = acknowledgment["observed_old_state"]
        require(observation["history_sha256"] == sha(canonical(self.plan["installed_history"]))
                and observation["catalog_sha256"] == sha(canonical(self.plan["installed_target_catalog"]))
                and observation["layout_sha256"] == sha(canonical(self.plan["installed_layout_slots"]))
                and observation["check_bindings_sha256"] == sha(canonical(self.plan["installed_check_bindings"]))
                and all(writer["running"] is self.plan["writer_running"][role]
                        for role, writer in observation["writers"].items()), "abort observation binding changed")
        return compose_abort_acknowledgment(raw, expected_bindings=self.abort_bindings(result,
            acknowledging_helper=acknowledgment["acknowledging_helper_sha256"], observation=observation,
            observed_at=acknowledgment["observed_at"]), compose_file=self.compose_file)

    def acknowledge_old_availability(self):
        self.guard_latest_stage()
        require(self.phase() == "backup-verified", "only an unapplied plain-C backup can be acknowledged")
        result, fence = self.old_availability_evidence(self.args.operator_result,
            self.args.operator_result_signature, self.args.verification_key)
        observation = self.old_availability_observation(result["state"], fence)
        names = ("abort-operator-result.json", "abort-operator-result.sig", "abort-acknowledgment.json")
        present = [os.path.lexists(self.stage / name) for name in names]
        if any(present):
            require(all(present)
                    and read(self.stage / names[0]) == read(self.args.operator_result)
                    and read(self.stage / names[1]) == read(self.args.operator_result_signature),
                    "partial or conflicting abort evidence requires diagnosis")
            acknowledgment = self.validate_abort_history()
            require(acknowledgment["observed_old_state"] == observation
                    and acknowledgment["acknowledging_helper_sha256"] == helper_identity(),
                    "prepared abort observation changed")
        else:
            write(self.stage / names[0], read(self.args.operator_result))
            write(self.stage / names[1], read(self.args.operator_result_signature))
            bindings = self.abort_bindings(result, acknowledging_helper=helper_identity(), observation=observation,
                                          observed_at=datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"))
            acknowledgment = {**bindings, "schema": "piping.compose-abort/v1", "terminal": ABORTED_OLD_AVAILABILITY,
                              "upgrade_accepted": False, "snapshot_historical": True}
            compose_abort_acknowledgment(canonical(acknowledgment), expected_bindings=bindings,
                                         compose_file=self.compose_file)
            write_json(self.stage / names[2], acknowledgment)
            self.validate_abort_history()
        self.advance(ABORTED_OLD_AVAILABILITY)

    def guard_latest_stage(self):
        stages = [path.name for path in (self.root / "backups").iterdir()
                  if re.fullmatch(r"release-[0-9]{8}T[0-9]{6}Z", path.name)]
        require(stages and max(stages) == self.stage.name, "a newer stage supersedes this operation")

    def guard_config(self, *, installed=False):
        self.guard_active_overlay()
        self.guard_runner_roots()
        if "job_network_activation_sha256" in self.plan:
            activation = self.guarded_activation()
            actual = (file_sha(self.job_network_directory() / JOB_NETWORK_ACTIVATION_FILE)
                      if activation else None)
            require(actual == self.plan["job_network_activation_sha256"],
                    "guarded job network activation changed since staging")
        for name, expected in self.plan["baseline"].items():
            if installed and name == self.plan["image_file"]:
                expected = file_sha(self.stage / "images.next")
            elif installed and name in self.plan.get("candidate_config", {}):
                candidate = self.plan["candidate_config"][name]
                require(isinstance(candidate, str) and Path(candidate).name == candidate,
                        "invalid staged candidate filename")
                expected = file_sha(self.stage / candidate)
            require(
                file_sha(self.root / name) == expected,
                "configuration changed since staging",
            )
        model_digest = self.plan.get("candidate_model_sha256" if installed else "original_model_sha256")
        if model_digest is not None:
            require(sha(canonical(json.loads(run(self.compose() + ["config", "--format", "json"])))) == model_digest,
                    "resolved Compose configuration changed since staging")

    def image(self, role):
        return self.plan["images"][role]["local_id"]

    def source_migration(self, name):
        require(isinstance(name, str) and Path(name).name == name
                and "apps/api/migrations/" + name in self.plan["prefix_manifest_sha384"],
                "unknown source migration")
        content = read(self.stage / "source-migrations" / name)
        require(hashlib.sha384(content).hexdigest() == self.plan["prefix_manifest_sha384"]["apps/api/migrations/" + name],
                "staged migration source changed")
        return content

    def inspect_images(self):
        verified = {}
        for role, image in self.plan["images"].items():
            require(re.fullmatch(r"sha256:[0-9a-f]{64}", image["local_id"]), "invalid local image ID")
            identity = (image["local_id"], image["config_digest"])
            if identity not in verified:
                verified[identity] = exported_config(image["local_id"], image["config_digest"], self.stage)
            config = verified[identity]
            require("/".join(filter(None, (config["os"], config["architecture"], config.get("variant", ""))))
                    == self.plan["platform"]
                    and (config["config"].get("Labels") or {}).get("org.opencontainers.image.revision")
                     == image["revision"], "offline local image identity mismatch")


    def published_migrations(self, ref=None):
        name = self.args.project_name + "-" + self.stage.name + "-source"
        require(
            not run(
                [
                    "docker",
                    "ps",
                    "-a",
                    "--filter",
                    "name=^/" + name + "$",
                    "--format",
                    "{{.Names}}",
                ]
            ),
            "source container already exists",
        )
        run(
            [
                "docker",
                "create",
                "--name",
                name,
                "--platform",
                self.plan["platform"],
                "--label",
                "piping.release-stage=" + self.stage.name,
                ref or self.image("backend"),
            ]
        )
        try:
            archive = run(["docker", "cp", name + ":/app/migrations", "-"])
            prefix = source_prefix_manifest(archive)
            if ref is None:
                require(self.plan.get("prefix_manifest_sha384", prefix) == prefix, "published source prefix paths changed")
                self.plan["prefix_manifest_sha384"] = prefix
                require("apps/api/migrations/0001_init.sql" in prefix, "accepted baseline identity changed")
                source_dir = self.stage / "source-migrations"
                if not os.path.lexists(source_dir):
                    source_dir.mkdir(mode=0o700)
                checked_path(source_dir, directory=True)
                require(stat.S_IMODE(source_dir.stat().st_mode) == 0o700, "migration source directory must be private")
                with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as bundle:
                    for member in bundle:
                        source_name = PurePosixPath(member.name).name
                        key = "apps/api/migrations/" + source_name
                        if not member.isfile() or key not in prefix:
                            continue
                        content = bundle.extractfile(member).read()
                        require(hashlib.sha384(content).hexdigest() == prefix[key], "published migration bytes changed")
                        path = source_dir / source_name
                        if os.path.lexists(path):
                            require(read(path) == content, "staged migration source changed")
                        else:
                            write(path, content)
            else:
                expected = {path: checksum for path, checksum in self.plan["prefix_manifest_sha384"].items()
                            if int(PurePosixPath(path).name.split("_", 1)[0]) <= len(self.plan["installed_history"])}
                require(prefix == expected, "installed backend source prefix identity changed")
            return migration_artifact(archive)
        finally:
            run(["docker", "rm", name])

    def stage_release(self):
        self.guard_recovery()
        args = self.args
        package = verify_package(args.package, args.verification_key)
        disabled = args.disabled_schedule
        if disabled is not None:
            require(str(uuid.UUID(disabled)) == disabled, "schedule UUID is not canonical")
        images = package["images"]
        require(args.image_file in (".env", ".images.env"), "explicit image assignment file required")
        config_names = self.config_names()
        if args.image_file == ".images.env" or os.path.lexists(self.root / ".images.env"):
            config_names.append(".images.env")
        originals = {name: read(self.root / name) for name in config_names}
        active_overlay = self.active_overlay_input()
        activation = self.guarded_activation()
        backups = self.root / "backups"
        if not os.path.lexists(backups):
            backups.mkdir(mode=0o700)
            sync_directory(self.root)
        checked_path(backups, directory=True)
        require(
            stat.S_IMODE(backups.stat().st_mode) == 0o700,
            "backup directory must be private",
        )
        checked_path(self.stage, missing=True)
        self.stage.mkdir(mode=0o700)
        (self.stage / "journal").mkdir(mode=0o700)
        sync_directory(backups)
        self.advance("staging")
        self.plan = {
            "root": str(self.root),
            "project_name": args.project_name,
            "commit": package["commit"],
            "pipeline_id": package["pipeline_id"],
            "platform": package["platform"],
            "helper_sha256": helper_identity(),
            "package_sha256": file_sha(args.package / "package.json"),
            "package_binding_sha256": package_binding(package),
            "release_sha256": package["release_sha256"],
            "target": socket.gethostname(),
            "preservation_sha256": package.get("preservation", {}).get("manifest_sha256"),
            "check_capture_sha256": package.get("preservation", {}).get("capture_sha256"),
            "verification_key_sha256": file_sha(args.verification_key),
            "disabled_schedule": disabled,
            "images": images,
            "image_file": args.image_file,
            "compose_file": self.compose_path().name,
            "job_network_activation_sha256": (
                file_sha(self.job_network_directory() / JOB_NETWORK_ACTIVATION_FILE)
                if activation else None),
            "candidate_config": {},
            "active_overlay": active_overlay,
        }
        if active_overlay:
            write(self.stage / "active-overlay.original", read(Path(active_overlay["path"])))
        candidate_directory = getattr(args, "runner_candidate", None)
        candidate_inputs = self.runner_candidate_inputs(candidate_directory, originals) if candidate_directory else {}
        for name, staged in ((ENV_FILE, "environment.next"), (self.compose_path().name, "compose.next")):
            if name in candidate_inputs:
                write(self.stage / staged, candidate_inputs[name])
                self.plan["candidate_config"][name] = staged
        candidate = render_images(candidate_inputs.get(args.image_file, originals[args.image_file]),
                                  {role: image["local_id"] for role, image in images.items()})
        write(self.stage / "verification-key.pem", read(args.verification_key))
        write(self.stage / "package.json", read(args.package / "package.json"))
        write(self.stage / "package.sig", read(args.package / "package.sig"))
        if "preservation" in package:
            write(self.stage / "preservation.json", read(args.package / "preservation.json"))
        if self.plan["check_capture_sha256"]:
            write(self.stage / "check-capture.json", read(args.package / "check-capture.json"))
        self.inspect_images()
        self.plan["migrations"], self.plan["contract"], self.plan["contract_sha256"] = self.published_migrations()
        validate_contract(self.plan["contract"], self.plan["migrations"])
        self.plan["baseline"] = {}
        for name in config_names:
            content = originals[name]
            self.plan["baseline"][name] = sha(content)
            write(self.stage / (name + ".original"), content)
        write(self.stage / "images.next", candidate)
        # Compose run has no --no-build flag. A native reset overlay removes the
        # backend build recipe, so even a lost cached image cannot trigger a build.
        write(
            self.stage / "migrate.compose.yml",
            b"services:\n  backend:\n    build: !reset null\n",
        )
        self.guard_config()
        before = json.loads(run(self.compose() + ["config", "--format", "json"]))
        after = json.loads(run(self.compose(self.stage / "images.next") + ["config", "--format", "json"]))
        self.plan["original_model_sha256"] = sha(canonical(before))
        self.plan["candidate_model_sha256"] = sha(canonical(after))
        for service, role in (
            ("backend", "backend"),
            ("frontend", "frontend"),
            ("ansible-runner", "ansible-runner"),
            ("proxy", "backend"),
        ):
            if service == "proxy" and service not in after["services"]:
                continue
            require(
                after["services"][service]["image"] == self.image(role),
                "wrong Compose image consumer",
            )
            before["services"][service]["image"] = after["services"][service]["image"]
        if candidate_inputs:
            self.guard_runner_candidate(before, after)
        else:
            require(before == after, "Compose changed beyond product service images")
        migration_model = json.loads(
            run(
                self.compose(self.stage / "images.next")
                + [
                    "-f",
                    str(self.stage / "migrate.compose.yml"),
                    "config",
                    "--format",
                    "json",
                ]
            )
        )
        after["services"]["backend"].pop("build", None)
        require(migration_model == after, "migration overlay changed runtime configuration")
        self.plan["writer_ids"] = {}
        self.plan["writer_running"] = {}
        for service in WRITERS:
            if service in after["services"]:
                ids = run(self.compose() + ["ps", "-aq", service]).decode().splitlines()
                require(len(ids) <= 1, "ambiguous installed writer identity")
                if ids:
                    self.plan["writer_ids"][service] = ids[0]
                    self.plan["writer_running"][service] = json.loads(run(["docker", "inspect", ids[0]]))[0]["State"]["Running"]
        require({"backend", "ansible-runner"} <= set(self.plan["writer_ids"]), "required writer identity unavailable")
        self.guard_migration_consumer(after["services"]["backend"])
        self.plan["installed_history"] = self.guard_database()
        self.plan["installed_target_catalog"] = self.catalog()
        self.plan["installed_layout_slots"] = self.layout_slots()
        self.plan["installed_append_slots"] = self.append_slots()
        self.plan["installed_check_bindings"] = self.installed_check_bindings() if "preservation" in package else []
        self.plan["target_postgres_image"] = self.service_image("postgres")
        self.plan["target_backend_image"] = self.service_image("backend")
        self.plan["postgres_image"] = self.plan["target_postgres_image"]
        if len(self.plan["installed_history"]) < len(self.plan["migrations"]):
            self.plan["installed_backend_image"] = self.plan["target_backend_image"]
        snapshot = self.stage / "installed-target-schema.dump"
        checked_path(snapshot, missing=True)
        with snapshot.open("xb") as stream:
            os.chmod(snapshot, 0o600)
            run(self.postgres_command("sh", "-c", 'exec pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" --schema-only --format=custom'),
                output=stream, timeout=300)
            stream.flush()
            os.fsync(stream.fileno())
        self.plan["schema_snapshot_sha256"] = file_sha(snapshot)
        exact_target_preservation(self.plan["installed_target_catalog"], self.catalog())
        self.reference_proof(before["services"]["backend"])
        self.guard_database()
        self.guard_target_catalog()
        self.guard_writer_ids()
        self.guard_config()
        write_json(self.stage / "plan.json", self.plan)
        self.seal(
            "stage-seal.json",
            [
                "plan.json",
                "images.next",
                "migrate.compose.yml",
                "proof-backend.env",
                "installed-target-schema.dump",
                "verification-key.pem", "package.json", "package.sig",
                *(["preservation.json"] if "preservation" in package else []),
                *(["check-capture.json"] if self.plan["check_capture_sha256"] else []),
                *(name + ".original" for name in config_names),
                *self.plan["candidate_config"].values(),
                *(["active-overlay.original"] if active_overlay else []),
            ],
        )
        self.advance("staged")

    def postgres_command(self, *command):
        return self.compose() + ["exec", "-T", "postgres", *command]

    def sql(self, query, *, timeout=120):
        return run(
            self.postgres_command(
                "sh",
                "-c",
                'exec psql -X -qAt -v ON_ERROR_STOP=1 -v VERBOSITY=verbose -U "$POSTGRES_USER" -d "$POSTGRES_DB"',
            ),
            data=query.encode(), psql=True, timeout=timeout,
        ).strip()

    def history(self):
        return json.loads(self.sql(
            "SELECT coalesce(json_agg(row_to_json(h) ORDER BY version::bigint),'[]'::json) FROM "
            "(SELECT version::text,description,installed_on AT TIME ZONE 'UTC' AS installed_on,success,encode(checksum,'hex') AS checksum,"
            "execution_time FROM public._sqlx_migrations) h;"
        ))

    def guard_history(self, *, completed=False, preserve_installed=True):
        history = self.history()
        sizes = {len(self.plan["migrations"])}
        if not completed:
            sizes.update(range(self.plan["contract"]["installed_prefix"], len(self.plan["migrations"])))
        require(
            len(history) in sizes
            and all(row["success"] is True for row in history)
            and [row["version"] for row in history] == [str(v) for v in range(1, len(history) + 1)]
            and all(row["checksum"] == self.plan["migrations"].get(row["version"]) for row in history),
            "migration history is not the published successful cohort",
        )
        identities = {str(int(PurePosixPath(path).name.split("_", 1)[0])):
                      PurePosixPath(path).stem.split("_", 1)[1].replace("_", " ")
                      for path in self.plan["prefix_manifest_sha384"]}
        require(all(row["description"] == identities.get(row["version"]) for row in history),
                "migration history description differs from source identity")
        require(all(type(row["execution_time"]) is int and row["execution_time"] >= 0 for row in history)
                and all(isinstance(row["installed_on"], str) for row in history)
                and [row["installed_on"] for row in history] == sorted(row["installed_on"] for row in history),
                "migration history execution metadata is invalid or reordered")
        baseline = self.plan.get("installed_history")
        if preserve_installed and baseline is not None:
            require(history[:len(baseline)] == baseline and (completed or history == baseline),
                    "installed migration metadata changed")
        return history

    def guard_database(self, *, completed=False):
        history = self.guard_history(completed=completed)
        if self.plan["disabled_schedule"] is not None:
            require(self.sql("SELECT NOT enabled FROM public.playbook_schedule WHERE id='"
                            + self.plan["disabled_schedule"] + "'::uuid;") == b"t",
                    "schedule must remain disabled")
        return history

    def catalog(self):
        return catalog_objects(self.sql(CATALOG_SQL))

    def layout_slots(self):
        # Include dropped and terminal attributes, not just live columns.
        return json.loads(self.sql("SELECT coalesce(json_object_agg(name,slot),'{}'::json) FROM ("
            "SELECT n.nspname||'.'||c.relname AS name,coalesce(max(a.attnum),0) AS slot "
            "FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace "
            "LEFT JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 "
            "WHERE n.nspname NOT LIKE 'pg\\_%' ESCAPE '\\' AND n.nspname <> 'information_schema' "
            "AND c.relkind IN ('r','p','v','m','f') GROUP BY n.nspname,c.relname) slots;"))

    def append_slots(self):
        tables = {o["name"].rsplit(".", 1)[0] for o in self.plan.get("source_delta", {}).get("additions", [])
                  if o["kind"] == "column"}
        if not tables:
            return {}
        literal = lambda value: "'" + value.replace("'", "''") + "'"
        return json.loads(self.sql("SELECT coalesce(json_object_agg(name,slot),'{}'::json) FROM (SELECT n.nspname||'.'||c.relname AS name,"
            "max(a.attnum) AS slot FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid "
            "JOIN pg_namespace n ON n.oid=c.relnamespace WHERE a.attnum>0 AND n.nspname||'.'||c.relname IN ("
            + ",".join(literal(t) for t in sorted(tables)) + ") GROUP BY n.nspname,c.relname) slots;"))

    def installed_check_bindings(self):
        bindings = []
        forms = historical_check_forms()
        for relation in sorted({name.rsplit(".", 1)[0] for name in forms}):
            values = json.loads(self.sql(CHECK_BINDINGS_SQL % ("'" + relation.replace("'", "''") + "'")))
            bindings.extend(b for b in values if b["name"] in forms)
        return sorted(bindings, key=lambda b: b["name"])

    def guard_target_catalog(self):
        self.observe_target_catalog()
        if self.plan["preservation_sha256"]:
            self.approval()

    def observe_target_catalog(self):
        exact_target_preservation(self.plan["installed_target_catalog"], self.catalog())
        require(self.layout_slots() == self.plan["installed_layout_slots"],
                "target complete physical layout changed")
        require(self.append_slots() == self.plan["installed_append_slots"], "target physical append layout changed")
        if self.plan["preservation_sha256"]:
            require(self.installed_check_bindings() == self.plan["installed_check_bindings"],
                    "installed native CHECK binding changed")
            if self.plan["check_capture_sha256"]:
                self.guard_original_check_identity()

    def guard_original_check_identity(self):
        names = "(" + ",".join("'" + name.replace("'", "''") + "'" for name in historical_check_forms()) + ")"
        observed = json.loads(self.sql("SELECT json_agg(json_build_object('name',n.nspname||'.'||c.relname||'.'||k.conname,"
            "'conkey',k.conkey,'enforced',k.conenforced,'validated',k.convalidated,'relation_oid',k.conrelid::text,"
            "'namespace_oid',k.connamespace::text,'constraint_oid',k.oid::text,'original_conbin',k.conbin::text)) "
            "FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace "
            "WHERE n.nspname||'.'||c.relname||'.'||k.conname IN " + names))
        captured = read_json(self.stage / "check-capture.json")["original"]
        require(sorted(observed, key=lambda o: o["name"]) == sorted(captured, key=lambda o: o["name"]),
                "original CHECK identity is not the captured target")

    def target_additions(self, slots, catalog, source_slots):
        additions = json.loads(json.dumps(self.pending_delta()["additions"]))
        renames = (migration_contract.SCHEDULE_OWNER_RENAMES
                   if "ScheduleOwner0021" in self.transforms() else {})
        installed_columns = {obj["name"]: obj["definition"] for obj in catalog if obj["kind"] == "column"}
        removals = {obj["name"] for obj in self.pending_delta()["removals"] if obj["kind"] == "column"}
        for obj in additions:
            if obj["kind"] == "column":
                if obj["name"] in renames:
                    old = renames[obj["name"]]
                    require(old in removals and old in installed_columns,
                            "schedule owner rename lacks its sealed source column")
                    obj["definition"]["position"] = installed_columns[old]["position"]
                    continue
                table = obj["name"].rsplit(".", 1)[0]
                if table in source_slots:
                    require(table in slots, "source addition table lacks its proven terminal slot")
                    require(obj["definition"]["position"] > source_slots[table],
                            "source addition does not append to the native source table")
                    obj["definition"]["position"] += slots[table] - source_slots[table]
        return additions

    def pending_delta(self):
        require("source_delta" in self.plan, "native source migration delta is not proven")
        return self.plan["source_delta"]

    def target_layout_edges(self, catalog, slots, source_slots):
        delta = self.pending_delta()
        replacements = json.loads(json.dumps(delta["replacements"]))
        removals = json.loads(json.dumps(delta["removals"]))
        if self.workspace_cutover():
            return replacements, removals
        routes = migration_contract.native_column_routes(delta)
        columns = {obj["name"]: obj["definition"] for obj in catalog if obj["kind"] == "column"}
        for obj in replacements + removals:
            if obj["kind"] != "column" or (obj["kind"], obj["name"]) not in routes:
                continue
            target = columns.get(obj["name"])
            before = obj["before"]
            require(isinstance(target, dict) and isinstance(before, dict)
                    and {k: v for k, v in before.items() if k != "position"}
                    == {k: v for k, v in target.items() if k != "position"},
                    "source column definition differs outside its proven position")
            source_position = before["position"]
            before["position"] = target["position"]
            if "after" in obj:
                after_position = obj["after"]["position"]
                if after_position == source_position:
                    obj["after"]["position"] = target["position"]
                else:
                    table = obj["name"].rsplit(".", 1)[0]
                    require(table in slots and table in source_slots and after_position > source_slots[table],
                            "source replacement position is not a proven append")
                    obj["after"]["position"] += slots[table] - source_slots[table]
        return replacements, removals

    def candidate_layout_slots(self, slots, source_slots, source_candidate_slots):
        require(set(source_slots) <= set(slots), "layout source relation coverage changed")
        extras = set(slots) - set(source_slots)
        require(not extras.intersection(source_candidate_slots), "source relation collides with an approved extension")
        result = {table: slots[table] for table in extras}
        for table, after in source_candidate_slots.items():
            if table in source_slots:
                require(after >= source_slots[table], "source relation was physically rebuilt")
                result[table] = slots[table] + after - source_slots[table]
            else:
                result[table] = after
        return result

    def service_cutover(self):
        return service_cutover_pending(self.plan.get("prefix_manifest_sha384", {}),
                                       len(self.plan.get("installed_history", ())))

    def workspace_cutover(self):
        pending = (len(self.plan.get('installed_history', ())) == 15
                   and len(self.plan.get('migrations', ())) in (16, 17, 18, 19))
        if pending:
            self.workspace_delta()
        return pending

    def workspace_delta(self):
        delta = self.pending_delta()
        if len(self.plan.get('migrations', ())) in (17, 18, 19):
            return workspace_0016_part(delta)
        require(workspace_0016_delta(**delta), 'unrecognized workspace migration delta')
        return delta

    def guard_workspace_prepared(self, baseline):
        if not self.workspace_cutover():
            return
        workspace_0016_conservation(baseline, self.workspace_delta())
        require(self.sql("SELECT NOT EXISTS (SELECT FROM public.secret_usage_binding WHERE "
                         "consumer_type IN ('playbook_version:runtime_env','playbook_project'));") == b't',
                'workspace migration requires retired secret usages to be removed')

    def transforms(self):
        return pending_transforms(self.plan.get("prefix_manifest_sha384", {}),
                                  len(self.plan.get("installed_history", ())))

    def expectation_context(self, baseline):
        return {"helper_sha256": helper_identity(), "application_source": self.plan["commit"],
                "prefix_sha256": definition_hash(self.plan["prefix_manifest_sha384"]),
                "installed_history_sha256": definition_hash(self.plan["installed_history"]),
                "raw_fingerprint_sha256": sha(canonical(baseline)),
                "transforms": list(self.transforms())}

    def content_type_expectation(self, baseline):
        require(baseline["columns"].get(CONTENT_TYPE_TABLE) == ["id", "app_label", "model"],
                "unsupported content-type registry columns")
        ids = json.loads(self.sql("SELECT coalesce(json_agg(id::text ORDER BY id::text),'[]'::json) "
                                  "FROM public.inventory_content_type;"))
        keys = self.plan["content_type_seed_keys"]
        result = {"schema": "piping.content-type-seeds/v1", "original_ids": ids,
                  "original_digest": baseline["rows"][CONTENT_TYPE_TABLE], "native_keys": keys,
                  "native_keys_sha256": sha(canonical(keys))}
        self.validate_content_type_expectation(result, baseline)
        return result

    def validate_content_type_expectation(self, value, baseline):
        require(isinstance(value, dict) and set(value) == {
            "schema", "original_ids", "original_digest", "native_keys", "native_keys_sha256"}
            and value["schema"] == "piping.content-type-seeds/v1"
            and baseline["columns"].get(CONTENT_TYPE_TABLE) == ["id", "app_label", "model"],
            "invalid content-type expectation")
        ids, keys = value["original_ids"], value["native_keys"]
        require(isinstance(ids, list) and len(ids) <= 100000 and all(
            isinstance(item, str) and str(uuid.UUID(item)) == item for item in ids)
            and ids == sorted(set(ids)), "invalid original content-type identities")
        require(isinstance(keys, list) and len(keys) <= 100000 and all(
            isinstance(key, list) and len(key) == 2
            and all(isinstance(part, str) and len(part) <= 100 for part in key) for key in keys)
            and keys == [list(key) for key in sorted({tuple(key) for key in keys})]
            and keys == self.plan["content_type_seed_keys"]
            and value["native_keys_sha256"] == sha(canonical(keys)), "invalid native content-type keys")
        digest = baseline["rows"][CONTENT_TYPE_TABLE]
        match = re.fullmatch(r"([0-9]+):[0-9a-f]{64}", digest) if isinstance(digest, str) else None
        require(match is not None and int(match[1]) == len(ids) and value["original_digest"] == digest,
                "original content-type digest is not bound to the raw backup")

    def migration_expectation(self, baseline, *, restored=False):
        transforms = self.transforms()
        require(transforms, "no source-bound data transformation was requested")
        expected_raw = dict(baseline)
        if restored:
            expected_raw["catalog"] = self.plan["restored_target_catalog"]
        require(self.fingerprint() == expected_raw, "source changed before native data expectation")
        if "Service0003" in transforms:
            self.sql(SERVICE_CUTOVER_VALIDATE_SQL)
        if "FHRP0004" in transforms:
            self.sql(FHRP_CUTOVER_VALIDATE_SQL)
        content_types = self.content_type_expectation(baseline) if "ContentTypeSeeds" in transforms else None
        projected = self.fingerprint(project_transforms=True)
        expected_catalog = self.plan["restored_target_catalog"] if restored else baseline["catalog"]
        require(projected["catalog"] == expected_catalog and projected["history"] == baseline["history"],
                "source changed during native data expectation")
        removed = {SERVICE_MAPPING_TABLE} if "Service0003" in transforms else set()
        changed = set(SERVICE_TABLES) | removed if "Service0003" in transforms else set()
        if "FHRP0004" in transforms:
            changed.add(FHRP_TABLE)
        if "Tag0006" in transforms:
            tag_rows = self.tag_migration_projection(expected_raw)
            changed.update((*tag_cutover_fields(baseline["columns"]), TAG_CATALOG_TABLE))
            projected["rows"].update(tag_rows)
        added = set()
        if "Dataflow0017" in transforms:
            dataflow_rows = self.dataflow_migration_projection(expected_raw)
            removed.add(DATAFLOW_REMOVED_TABLE)
            changed.update((*DATAFLOW_CHANGED_TABLES, DATAFLOW_REMOVED_TABLE))
            added.update(DATAFLOW_NEW_TABLES)
            projected["rows"].update(dataflow_rows)
        if "Monitoring0020" in transforms:
            removed.add(migration_contract.MONITORING_LEGACY_TABLE)
            changed.add(migration_contract.MONITORING_LEGACY_TABLE)
            added.update(migration_contract.MONITORING_NEW_TABLES)
        if "ScheduleOwner0021" in transforms:
            changed.add(migration_contract.SCHEDULE_TABLE)
        untouched = set(baseline["rows"]) - changed
        require(set(projected["rows"]) == (set(baseline["rows"]) - removed) | added
                and all(projected["rows"][name] == baseline["rows"][name] for name in untouched),
                "native data expectation omitted or changed unrelated rows")
        require(self.fingerprint() == expected_raw, "source changed during native data expectation")
        return {"schema": "piping.migration-conservation/v1",
                "context": self.expectation_context(baseline),
                "columns": projected["columns"], "rows": projected["rows"], "content_type_seeds": content_types}

    def load_migration_expectation(self, baseline):
        value = read_json(self.stage / "migration-expectation.json")
        removed = {SERVICE_MAPPING_TABLE} if self.service_cutover() else set()
        added = set()
        changed = {*SERVICE_TABLES, SERVICE_MAPPING_TABLE} if self.service_cutover() else set()
        if "FHRP0004" in self.transforms():
            changed.add(FHRP_TABLE)
        if "Tag0006" in self.transforms():
            changed.update((*tag_cutover_fields(baseline["columns"]), TAG_CATALOG_TABLE))
        if "Dataflow0017" in self.transforms():
            removed.add(DATAFLOW_REMOVED_TABLE)
            added.update(DATAFLOW_NEW_TABLES)
            changed.update((*DATAFLOW_CHANGED_TABLES, DATAFLOW_REMOVED_TABLE))
        if "Monitoring0020" in self.transforms():
            removed.add(migration_contract.MONITORING_LEGACY_TABLE)
            changed.add(migration_contract.MONITORING_LEGACY_TABLE)
            added.update(migration_contract.MONITORING_NEW_TABLES)
        if "ScheduleOwner0021" in self.transforms():
            changed.add(migration_contract.SCHEDULE_TABLE)
        require(isinstance(value, dict) and set(value) == {"schema", "context", "columns", "rows", "content_type_seeds"}
                and value["schema"] == "piping.migration-conservation/v1"
                and value["context"] == self.expectation_context(baseline)
                and value["columns"] == transformation_columns(baseline["columns"], self.transforms())
                and set(value["rows"]) == (set(baseline["rows"]) - removed) | added
                and all(value["rows"][name] == baseline["rows"][name]
                        for name in set(baseline["rows"]) - changed),
                "data transformation expectation is not bound to the raw backup")
        if "Monitoring0020" in self.transforms():
            require(all(isinstance(value["rows"][table], str)
                        and re.fullmatch(r"[0-9]+:[0-9a-f]{64}", value["rows"][table])
                        for table in migration_contract.MONITORING_NEW_TABLES),
                    "invalid monitoring conservation digest")
        if "ContentTypeSeeds" in self.transforms():
            self.validate_content_type_expectation(value["content_type_seeds"], baseline)
        else:
            require(value["content_type_seeds"] is None, "unexpected content-type expectation")
        return value

    def tag_migration_projection(self, baseline):
        """Execute only pinned Tag SQL on a private pre-DDL restore, never the installation."""
        content = self.source_migration(PurePosixPath(TAG_CUTOVER_PATH).name)
        require(hashlib.sha384(content).hexdigest() == TAG_CUTOVER_SHA384,
                "canonical Tag0006 expectation source changed")
        fields = tag_cutover_fields(baseline["columns"])
        require(set(baseline["columns"].get(TAG_CATALOG_TABLE, ())) == set(TAG_FACTORY_ROW),
                "Tag0006 factory catalog has unknown or missing columns")
        other_columns = {table: [name for name in names if name != fields.get(table)]
                         for table, names in baseline["columns"].items()}
        active = getattr(self, "_active_database", None)
        dump = (active["compose"] + ["exec", "-T", "postgres", "pg_dump", "-Fc", "-U", "piping",
                                   "-d", active.get("name", "piping")] if active else
                self.postgres_command("sh", "-c", 'exec pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc'))
        with tempfile.TemporaryFile(dir=self.stage) as archive:
            run(dump, output=archive, timeout=300)
            archive.seek(0)
            with self.isolated_database("tag0006-expectation") as database:
                self.restore_archive(database, archive)
                with self.database_target(database):
                    before = self.fingerprint()
                    require(before == {**baseline, "catalog": self.plan["restored_target_catalog"]},
                            "Tag0006 private restore changed pre-DDL state")
                    require(self.layout_slots() == self.plan["restored_layout_slots"],
                            "Tag0006 private restore changed the physical layout")
                    native = json.loads(self.sql(
                        "SELECT coalesce(json_agg('public.' || r.relname ORDER BY r.relname),'[]'::json) "
                        "FROM pg_class r JOIN pg_namespace n ON n.oid=r.relnamespace "
                        "JOIN pg_attribute a ON a.attrelid=r.oid WHERE n.nspname='public' "
                        "AND r.relkind IN ('r','p') "
                        "AND r.relname ~ '^(circuits|dcim|ipam|tenancy|virtualization|vpn|wireless)_' "
                        "AND a.attname='tags' AND a.atttypid='text[]'::regtype AND NOT a.attisdropped;"))
                    require(set(native) == set(fields) - {TAG_SELECTOR_TABLE},
                            "Tag0006 membership owners differ from the sealed columns")
                    unrelated = self.fingerprint(other_columns)
                    catalog_rows_sql = (
                        "SELECT coalesce(jsonb_agg(to_jsonb(t)),'[]'::jsonb) FROM public.inventory_tag t;")
                    catalog_rows = json.loads(self.sql(catalog_rows_sql))
                    expected_catalog_rows = list(catalog_rows)
                    if not any(row["id"] == TAG_FACTORY_ROW["id"] for row in catalog_rows):
                        expected_catalog_rows.append(TAG_FACTORY_ROW)
                    # SQLx's single-transaction semantics, including all refusal paths.
                    self.sql("BEGIN;\n" + content.decode("utf-8") + "\nCOMMIT;")
                    require(sorted(canonical(row) for row in json.loads(self.sql(catalog_rows_sql)))
                            == sorted(canonical(row) for row in expected_catalog_rows),
                            "Tag0006 changed catalog rows beyond the exact factory seed")
                    after = self.fingerprint(other_columns)
                    # The complete catalog multiset above replaces only its old
                    # row hash; all its columns remain in the conserved inventory.
                    require(after["columns"] == unrelated["columns"]
                            and {name: value for name, value in after["rows"].items() if name != TAG_CATALOG_TABLE}
                            == {name: value for name, value in unrelated["rows"].items() if name != TAG_CATALOG_TABLE}
                            and after["history"] == unrelated["history"],
                            "Tag0006 changed a non-Tag field or ledger")
                    projected = self.fingerprint(project_transforms=True)
                    require(projected["columns"] == transformation_columns(baseline["columns"], self.transforms()),
                            "Tag0006 changed the conserved column inventory")
                    return {table: projected["rows"][table] for table in (*fields, TAG_CATALOG_TABLE)}

    def dataflow_relation_checks(self):
        """Prove the selector/selected-IP relation is complete and bijective."""
        checks = (
            ("Dataflow0017 did not keep exactly two ordered endpoints per flow",
             "SELECT NOT EXISTS (SELECT 1 FROM public.inventory_dataflow f "
             "JOIN public.inventory_dataflow_selector s ON s.dataflow_id=f.id GROUP BY f.id "
             "HAVING count(*)<>2 OR count(*) FILTER (WHERE s.side='source')<>1 "
             "OR count(*) FILTER (WHERE s.side='destination')<>1 "
             "OR min(s.position)<>1 OR max(s.position)<>1);"),
            ("Dataflow0017 selected mode and selected IPs are inconsistent",
             "SELECT NOT EXISTS (SELECT 1 FROM public.inventory_dataflow_selector s "
             "WHERE s.dataflow_id IS NOT NULL AND ((s.address_mode='selected') <> EXISTS "
             "(SELECT 1 FROM public.inventory_dataflow_selector_ip n WHERE n.selector_id=s.id)));"),
            ("Dataflow0017 address-group members are not one expansion level",
             "SELECT NOT EXISTS (SELECT 1 FROM public.inventory_dataflow_selector WHERE group_id IS NOT NULL);"),
            ("Dataflow0017 selector identity is not unique",
             "SELECT count(*)=count(DISTINCT id) FROM public.inventory_dataflow_selector;"),
            ("Dataflow0017 selected-IP identity is not unique",
             "SELECT count(*)=count(DISTINCT (selector_id,ipaddress_id)) "
             "FROM public.inventory_dataflow_selector_ip;"),
            ("Dataflow0017 selector and selected-IP relation is not bijective",
             "SELECT count(*)=count(DISTINCT (s.dataflow_id,s.side,s.position,n.ipaddress_id)) "
             "FROM public.inventory_dataflow_selector_ip n "
             "JOIN public.inventory_dataflow_selector s ON s.id=n.selector_id;"),
        )
        for message, query in checks:
            require(self.sql(query) == b"t", message)

    def dataflow_migration_projection(self, baseline):
        """Execute only pinned 0017 SQL on a private pre-DDL restore, never the installation."""
        content = self.source_migration(PurePosixPath(DATAFLOW_CUTOVER_PATH).name)
        require(hashlib.sha384(content).hexdigest() == DATAFLOW_CUTOVER_SHA384,
                "canonical Dataflow0017 expectation source changed")
        active = getattr(self, "_active_database", None)
        dump = (active["compose"] + ["exec", "-T", "postgres", "pg_dump", "-Fc", "-U", "piping",
                                   "-d", active.get("name", "piping")] if active else
                self.postgres_command("sh", "-c", 'exec pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc'))
        with tempfile.TemporaryFile(dir=self.stage) as archive:
            run(dump, output=archive, timeout=300)
            archive.seek(0)
            with self.isolated_database("dataflow0017-expectation") as database:
                self.restore_archive(database, archive)
                with self.database_target(database):
                    before = self.fingerprint()
                    require(before == {**baseline, "catalog": self.plan["restored_target_catalog"]},
                            "Dataflow0017 private restore changed pre-DDL state")
                    require(self.layout_slots() == self.plan["restored_layout_slots"],
                            "Dataflow0017 private restore changed the physical layout")
                    flow_before = json.loads(self.sql(DATAFLOW_FLOW_STATE_SQL))
                    endpoint_before = json.loads(self.sql(DATAFLOW_ENDPOINT_STATE_PRE_SQL))
                    selected_before = json.loads(self.sql(DATAFLOW_SELECTED_STATE_PRE_SQL))
                    content_before = {tuple(key) for key in json.loads(self.sql(DATAFLOW_CONTENT_TYPE_KEYS_SQL))}
                    # SQLx's single-transaction semantics, including all refusal paths.
                    self.sql("BEGIN;\n" + content.decode("utf-8") + "\nCOMMIT;")
                    require(json.loads(self.sql(DATAFLOW_FLOW_STATE_SQL)) == flow_before,
                            "Dataflow0017 changed flow identity, status or traffic")
                    require(json.loads(self.sql(DATAFLOW_ENDPOINT_STATE_POST_SQL)) == endpoint_before,
                            "Dataflow0017 changed endpoint kind, mode or typed reference")
                    require(json.loads(self.sql(DATAFLOW_SELECTED_STATE_POST_SQL)) == selected_before,
                            "Dataflow0017 changed selected-IP associations")
                    content_after = {tuple(key) for key in json.loads(self.sql(DATAFLOW_CONTENT_TYPE_KEYS_SQL))}
                    require(content_after - content_before == set(DATAFLOW_CONTENT_TYPE_SEEDS)
                            and content_before <= content_after,
                            "Dataflow0017 seeded unexpected content types")
                    self.dataflow_relation_checks()
                    projected = self.fingerprint()
                    require(projected["columns"] == transformation_columns(baseline["columns"], ("Dataflow0017",)),
                            "Dataflow0017 changed the conserved column inventory")
                    require(projected["history"] == baseline["history"],
                            "Dataflow0017 changed the SQLx ledger")
                    untouched = set(baseline["rows"]) - set(DATAFLOW_CHANGED_TABLES) - {
                        DATAFLOW_REMOVED_TABLE, CONTENT_TYPE_TABLE}
                    require(all(projected["rows"][table] == baseline["rows"][table] for table in untouched),
                            "Dataflow0017 changed unrelated rows")
                    return {table: projected["rows"][table] for table in DATAFLOW_CHANGED_TABLES}

    def preservation_context(self):
        return {"target": self.plan["target"], "root": self.plan["root"], "project_name": self.plan["project_name"],
                "application_source": self.plan["commit"], "helper_sha256": self.plan["helper_sha256"],
                "release_sha256": self.plan["release_sha256"], "postgres_image": self.plan["target_postgres_image"],
                "backend_image": self.plan["target_backend_image"],
                "ledger_sha256": definition_hash(self.plan["installed_history"]),
                "prefix_sha256": definition_hash(self.plan["prefix_manifest_sha384"]),
                "installed_catalog_sha256": definition_hash(self.plan["installed_target_catalog"])}

    def approval(self):
        if self.plan["preservation_sha256"] is None:
            return None
        require(file_sha(self.stage / "preservation.json") == self.plan["preservation_sha256"]
                and (self.plan["check_capture_sha256"] is None
                     or file_sha(self.stage / "check-capture.json") == self.plan["check_capture_sha256"]),
                "preservation authority bytes changed")
        value = preservation_manifest(read_json(self.stage / "preservation.json"), context=self.preservation_context(),
                                      package_sha256=self.plan["package_binding_sha256"])
        require(value["check_capture_sha256"] == self.plan["check_capture_sha256"]
                and value["append_slots"] == self.plan["installed_append_slots"], "preservation layout/capture mismatch")
        return value

    @contextlib.contextmanager
    def native_sql_session(self, database):
        """Native proof TEMP tables require one real, bounded psql session."""
        with tempfile.TemporaryFile() as errors:
            process = subprocess.Popen(database["compose"] + ["exec", "-T", "postgres", "psql", "-X", "-qAt",
                "-v", "ON_ERROR_STOP=1", "-U", "piping", "-d", database.get("name", "piping")],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors, env=environment())
            selector = selectors.DefaultSelector()
            selector.register(process.stdout, selectors.EVENT_READ)
            buffered = b""
            def sql(query):
                nonlocal buffered
                marker = ("proof_" + uuid.uuid4().hex).encode()
                process.stdin.write(query.encode() + b";\n\\echo " + marker + b"\n")
                process.stdin.flush()
                end = time.monotonic() + 120
                while b"\n" + marker + b"\n" not in b"\n" + buffered:
                    require(time.monotonic() < end and selector.select(max(0, end-time.monotonic())),
                            "native proof SQL deadline")
                    chunk = os.read(process.stdout.fileno(), 65536)
                    require(chunk and len(buffered) + len(chunk) <= MAX_BYTES, "native proof SQL failed or exceeded bound")
                    buffered += chunk
                output, buffered = (b"\n" + buffered).split(b"\n" + marker + b"\n", 1)
                return output.strip().decode()
            try:
                yield sql
            finally:
                selector.close()
                process.stdin.close()
                if process.poll() is None:
                    process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=15)
                process.stdout.close()

    def layout_profile(self, catalog, slots, family, checkpoint=None):
        prefix = len(self.plan["installed_history"])
        manifest = {path: digest for path, digest in self.plan["prefix_manifest_sha384"].items()
                    if int(PurePosixPath(path).name.split("_", 1)[0]) <= prefix}
        return {"family": family, "checkpoint": checkpoint,
                "prefix_sha256": definition_hash(manifest), "prefix": prefix,
                "backend_image": self.plan.get("installed_backend_image") or self.image("backend"),
                "postgres_image": self.plan["target_postgres_image"],
                "columns": {o["name"]: o["definition"]["position"] for o in catalog if o["kind"] == "column"},
                "slots": slots}

    def layout_matches_installation(self, profile):
        catalog = self.plan["installed_target_catalog"]
        columns = {o["name"]: o["definition"]["position"] for o in catalog if o["kind"] == "column"}
        if any(columns.get(name) != position for name, position in profile["columns"].items()):
            return False
        approval = self.approval()
        inventory = {(item["kind"], item["name"]) for item in approval["inventory"]} if approval else set()
        extra_columns = set(columns) - set(profile["columns"])
        if any(("column", name) not in inventory for name in extra_columns):
            return False
        slots = dict(profile["slots"])
        for obj in catalog:
            if (obj["kind"] == "relation" and obj["definition"]["kind"] in ("r", "p", "v", "m", "f")
                    and obj["name"] not in slots):
                if ("relation", obj["name"]) not in inventory:
                    return False
                slots[obj["name"]] = 0
        for name in extra_columns:
            table = name.rsplit(".", 1)[0]
            if table not in slots or type(columns[name]) is not int:
                return False
            slots[table] = max(slots[table], columns[name])
        return slots == self.plan["installed_layout_slots"]

    def intermediate_layout(self, database, source_catalog, source_slots, checkpoint=None):
        """Replay pinned source SQL and native restores; never invent SQLx ledger rows."""
        prefix = len(self.plan["installed_history"])
        require(prefix > 1 and (checkpoint is None or type(checkpoint) is int and 1 <= checkpoint < prefix),
                "invalid native intermediate layout checkpoint")
        names = {int(PurePosixPath(path).name.split("_", 1)[0]): PurePosixPath(path).name
                 for path in self.plan["prefix_manifest_sha384"]}
        require(set(range(1, prefix + 1)) <= set(names), "missing native source prefix")
        with self.database_target(database):
            self.sql("CREATE DATABASE source_layout_raw;")
        raw = {**database, "name": "source_layout_raw"}
        with contextlib.ExitStack() as stack:
            metadata = stack.enter_context(tempfile.TemporaryFile(dir=self.stage))
            run(database["compose"] + ["exec", "-T", "postgres", "pg_dump", "-Fc", "--schema-only",
                "--no-owner", "--no-privileges", "-t", "public._sqlx_migrations", "-U", "piping",
                "-d", database.get("name", "piping")], output=metadata, timeout=300)
            metadata.seek(0)
            self.restore_archive(raw, metadata)
            snapshots = {}
            for version in range(1, prefix + 1):
                content = self.source_migration(names[version]).decode("utf-8")
                with self.database_target(raw):
                    self.sql("BEGIN;\n" + content + "\nCOMMIT;", timeout=300)
                if version < prefix and (checkpoint is None or checkpoint == version):
                    archive = stack.enter_context(tempfile.TemporaryFile(dir=self.stage))
                    run(raw["compose"] + ["exec", "-T", "postgres", "pg_dump", "-Fc", "--no-owner",
                        "--no-privileges", "-U", "piping", "-d", raw["name"]], output=archive, timeout=300)
                    snapshots[version] = archive
            # A derived checkpoint is not authority until its raw replay is
            # controlled against the actual compiled prefix, including slots.
            with self.database_target(raw):
                exact_target_preservation(source_catalog, self.catalog())
                require(self.layout_slots() == source_slots, "source SQL replay physical layout differs from compiled prefix")
            for version in sorted(snapshots, reverse=True):
                with self.database_target(database):
                    self.sql("CREATE DATABASE source_layout_compact;")
                compact = {**database, "name": "source_layout_compact"}
                archive = snapshots[version]
                archive.seek(0)
                self.restore_archive(compact, archive)
                with self.database_target(compact):
                    for later in range(version + 1, prefix + 1):
                        content = self.source_migration(names[later]).decode("utf-8")
                        self.sql("BEGIN;\n" + content + "\nCOMMIT;", timeout=300)
                    profile = self.layout_profile(self.catalog(), self.layout_slots(), "intermediate", version)
                with self.database_target(database):
                    self.sql("DROP DATABASE source_layout_compact;")
                if self.layout_matches_installation(profile):
                    return profile
        raise DeployError("installed layout is not a source-proven native family")

    def source_readiness(self, canonical_catalog, restored_catalog, source_checks, database, source_slots, restored_slots):
        approval = self.approval()
        installed_catalog = self.plan["installed_target_catalog"]
        def columns(catalog):
            return {o["name"]: o["definition"]["position"] for o in catalog if o["kind"] == "column"}
        raw_columns, installed_columns = map(columns, (canonical_catalog, installed_catalog))
        profiles = [self.layout_profile(canonical_catalog, source_slots, "raw"),
                    self.layout_profile(restored_catalog, restored_slots, "roundtrip")]
        pairs = self.plan["contract"]["position_pairs"]
        if (all(raw_columns.get(name) == pair[0] for name, pair in pairs.items())
                and any(all(installed_columns.get(name) == pair[orientation] for name, pair in pairs.items())
                        for orientation in (0, 1))):
            baseline = baseline_positions(self.plan["contract"], canonical_catalog, installed_catalog)
            translated = json.loads(json.dumps(canonical_catalog))
            for obj in translated:
                if obj["kind"] == "column" and obj["name"] in baseline:
                    obj["definition"]["position"] = baseline[obj["name"]][1]
            profiles.append(self.layout_profile(translated, baseline_append_slots(canonical_catalog, source_slots, baseline),
                                                "baseline"))
        # A pre-baseline database keeps the canonical definitions but the closed
        # rebuilt relations follow the source replay order compactly. This
        # complete vector is source-derived and offered only when an approved
        # operator preservation manifest binds this exact target catalog.
        if approval is not None:
            rebuilt_catalog, rebuilt_slots = source_rebuilt_layout(canonical_catalog, source_slots)
            rebuilt = self.layout_profile(rebuilt_catalog, rebuilt_slots, "source-rebuilt")
            if rebuilt["columns"] != raw_columns:
                profiles.append(rebuilt)
        sealed = self.plan.get("source_layout")
        profile = next((candidate for candidate in profiles if self.layout_matches_installation(candidate)
                        and (sealed is None or candidate == sealed)), None)
        if profile is None:
            checkpoint = sealed["checkpoint"] if sealed is not None and sealed.get("family") == "intermediate" else None
            require(sealed is None or checkpoint is not None, "sealed source layout cannot be rederived")
            profile = self.intermediate_layout(database, canonical_catalog, source_slots, checkpoint)
        require(sealed is None or profile == sealed, "sealed source layout differs from native source")
        self.plan["source_layout"] = profile
        positions = {name: [position, profile["columns"][name]] for name, position in raw_columns.items()
                     if position != profile["columns"][name]}
        if approval:
            for name, pair in approval["position_pairs"].items():
                require(name in raw_columns and list(pair) == [raw_columns[name], profile["columns"][name]],
                        "approved positions differ from the native source layout")
                if pair[0] != pair[1]:
                    positions[name] = pair
        # Only source-owned native CHECK reparses can change the canonical side.
        installed_definitions = {(o["kind"], o["name"]): o["definition"] for o in installed_catalog}
        canonical_catalog = json.loads(json.dumps(canonical_catalog))
        for obj in canonical_catalog:
            pair = source_checks.get(obj["name"]) if obj["kind"] == "constraint" else None
            if (pair is not None and obj["definition"] == pair[0]
                    and installed_definitions.get(("constraint", obj["name"])) == pair[1]):
                obj["definition"] = pair[1]
        proofs, bindings = (), ()
        if approval and approval["check_capture_sha256"] is not None:
            capture = read(self.stage / "check-capture.json", maximum=262144)
            require(sorted(json.loads(capture)["bindings"], key=lambda b: b["name"])
                    == self.plan["installed_check_bindings"], "approved CHECK capture differs from installed target")
            with self.native_sql_session(database) as sql:
                proofs = prove_original_native_checks(sql, capture, trusted_capture_sha256=approval["check_capture_sha256"],
                    source_sha=self.plan["commit"], postgres_image=self.plan["target_postgres_image"],
                    prefix_sha384=self.plan["prefix_manifest_sha384"])
            bindings = json.loads(capture)["bindings"]
            canonical_definitions = {(o["kind"], o["name"]): o["definition"] for o in canonical_catalog}
            installed_definitions = {(o["kind"], o["name"]): o["definition"] for o in self.plan["installed_target_catalog"]}
            proofs = tuple(p for p in proofs if canonical_definitions[("constraint", p.name)]
                           != installed_definitions[("constraint", p.name)])
        try:
            canonical_readiness(canonical_catalog, self.plan["installed_target_catalog"],
                trusted_inventory=approval["inventory"] if approval else (),
                position_pairs=positions,
                check_proofs=proofs, installed_check_bindings=bindings,
                source_sha=self.plan["commit"], prefix_sha384=self.plan["prefix_manifest_sha384"],
                postgres_image=self.plan["target_postgres_image"])
        except ValueError:
            before = {(o["kind"], o["name"]): o["definition"] for o in canonical_catalog}
            after = {(o["kind"], o["name"]): o["definition"] for o in self.plan["installed_target_catalog"]}
            write_json(self.stage / "readiness-difference.private.json", [
                {"kind": key[0], "name": key[1], "expected": before.get(key), "actual": after.get(key)}
                for key in sorted(set(before) | set(after)) if before.get(key) != after.get(key)])
            raise

    def roundtrip_target(self, database):
        """Permit only deterministic native compaction and proven CHECK reparse."""
        before, after = self.plan["installed_target_catalog"], self.catalog()
        observed = {(o["kind"], o["name"]): o for o in after}
        expected = json.loads(json.dumps(before))
        by_table = {}
        for obj in expected:
            if obj["kind"] == "column":
                by_table.setdefault(obj["name"].rsplit(".", 1)[0], []).append(obj)
        for columns in by_table.values():
            for position, obj in enumerate(sorted(columns, key=lambda o: o["definition"]["position"]), 1):
                obj["definition"]["position"] = position
        approved = self.approval()
        forms = historical_check_forms() if approved and approved["check_capture_sha256"] is not None else {}
        with self.native_sql_session(database) as sql:
            sql("SET search_path=pg_catalog,public")
            for obj in expected:
                key = obj["kind"], obj["name"]
                if key in observed and obj["kind"] == "constraint" and obj["definition"] != observed[key]["definition"]:
                    source_pair = self.plan["source_roundtrip_checks"].get(obj["name"])
                    if source_pair == [obj["definition"], observed[key]["definition"]]:
                        obj["definition"] = source_pair[1]
                        continue
                    require(obj["name"] in forms, "unapproved native CHECK roundtrip")
                    relation = obj["name"].rsplit(".", 1)[0]
                    bindings = json.loads(sql(CHECK_BINDINGS_SQL % ("'" + relation.replace("'", "''") + "'")))
                    binding = next(b for b in bindings if b["name"] == obj["name"])
                    prove_native_check(sql, forms[obj["name"]], binding, source_sha=self.plan["commit"],
                        postgres_image=self.plan["target_postgres_image"],
                        prefix_sha384=self.plan["prefix_manifest_sha384"])
                    obj["definition"] = binding["definition"]
        exact_target_preservation(expected, after)
        return after

    def guard_migration_consumer(self, backend):
        image = json.loads(run(["docker", "image", "inspect", self.image("backend")]))[0]["Config"]
        require(not any(item.split("=", 1)[0] in {"PIPING_MIGRATIONS_DIR", "LD_PRELOAD", "LD_LIBRARY_PATH"}
                        for item in image.get("Env", [])), "image source environment override refused")
        for field, native in (("entrypoint", "Entrypoint"), ("command", "Cmd")):
            value = backend.get(field)
            require(value is None or value == image.get(native), "backend executable override refused")
        require(not backend.get("privileged") and not backend.get("devices")
                and not backend.get("cap_add"), "unsafe backend migration privileges")
        require(not {"PIPING_MIGRATIONS_DIR", "LD_PRELOAD", "LD_LIBRARY_PATH"}.intersection(backend.get("environment", {})),
                "backend source environment override refused")
        require(backend.get("environment", {}).get("PIPING_RUN_MODE", "backend") == "backend",
                "migration consumer is not backend mode")
        url = urlsplit(backend.get("environment", {}).get("PIPING_DATABASE__URL", ""))
        target = json.loads(self.sql("SELECT json_build_object('database',current_database(),'user',current_user);"))
        require(url.scheme in ("postgres", "postgresql") and url.hostname == "postgres"
                and url.port in (None, 5432) and unquote(url.path.removeprefix("/")) == target["database"]
                and unquote(url.username or "") == target["user"] and not url.query and not url.fragment,
                "backend migration target differs from guarded PostgreSQL service")
        require(not backend.get("network_mode") and not backend.get("volumes_from"),
                "backend migration bypasses owned Compose resources")
        hosts = backend.get("extra_hosts", {})
        require(isinstance(hosts, dict) and "postgres" not in hosts, "backend PostgreSQL host override refused")
        for volume in backend.get("volumes", []):
            target = PurePosixPath(volume["target"])
            require(target.is_absolute() and ".." not in target.parts, "unsafe backend mount")
            for protected in (PurePosixPath("/app/migrations"), PurePosixPath("/usr/local"), PurePosixPath("/bin"),
                              PurePosixPath("/usr/bin"), PurePosixPath("/lib"), PurePosixPath("/lib64"),
                              PurePosixPath("/usr/lib"), PurePosixPath("/etc/ld.so.preload"),
                              PurePosixPath("/var/run/docker.sock")):
                require(target != protected and target not in protected.parents and protected not in target.parents,
                        "backend source or executable mount refused")

    def service_image(self, service):
        ids = run(self.compose() + ["ps", "-q", service]).decode().splitlines()
        require(len(ids) == 1, "installed service identity unavailable")
        inspected = json.loads(run(["docker", "inspect", ids[0]]))
        require(len(inspected) == 1 and inspected[0]["State"]["Running"], "installed service is not running")
        image = inspected[0]["Image"]
        require(re.fullmatch(r"sha256:[0-9a-f]{64}", image), "invalid installed image identity")
        return image

    @contextlib.contextmanager
    def isolated_database(self, purpose):
        # Native Compose owns readiness; a unique private project never joins client networks.
        project = "piping-proof-" + uuid.uuid4().hex
        password = uuid.uuid4().hex
        config = self.stage / (purpose + "-" + project + ".compose.json")
        scratch_root = checked_path(self.stage, directory=True)
        scratch = Path(tempfile.mkdtemp(prefix=project + "-", dir=scratch_root))
        data_directory = scratch / "postgres"
        # PostgreSQL 18 creates/chowns its versioned PGDATA child, not the mount
        # root. Traversal here is safe inside the operator's 0700 scratch parent.
        data_directory.mkdir(mode=0o755)
        os.chmod(data_directory, 0o755)
        network = {"internal": True}
        proof_subnet = getattr(self.args, "proof_subnet", None)
        if proof_subnet is not None:
            pool = self.private_network(proof_subnet, prefix=24)
            index = len(tuple(self.stage.glob("proof-network-*.json")))
            subnets = list(pool.subnets(new_prefix=29))
            require(index < len(subnets), "owned proof subnet pool exhausted")
            subnet = str(subnets[index])
            write_json(self.stage / f"proof-network-{index:02d}.json", {"subnet": subnet})
            network["ipam"] = {"config": [{"subnet": subnet}]}
        write_json(config, {"networks": {"default": network}, "services": {"postgres": {
            "image": self.plan["postgres_image"],
            "environment": {"POSTGRES_USER": "piping", "POSTGRES_DB": "piping", "POSTGRES_PASSWORD": password},
            "volumes": [{"type": "bind", "source": str(data_directory), "target": "/var/lib/postgresql"}],
            "cpus": 1, "mem_limit": "768m",
            "healthcheck": {"test": ["CMD", "pg_isready", "-h", "127.0.0.1", "-U", "piping", "-d", "piping"],
                            "interval": "1s", "timeout": "3s", "retries": PROOF_POSTGRES_HEALTH_RETRIES,
                            "start_period": str(PROOF_POSTGRES_READY_SECONDS - PROOF_POSTGRES_HEALTH_RETRIES) + "s"},
        }}})
        compose = ["docker", "compose", "-p", project, "-f", str(config)]
        try:
            run(compose + ["up", "-d", "--wait", "--wait-timeout", str(PROOF_POSTGRES_READY_SECONDS),
                           "--pull", "never", "--no-build"], timeout=PROOF_POSTGRES_READY_SECONDS + 30)
            yield {
                "compose": compose, "network": project + "_default",
                "url": "postgresql://piping:" + password + "@postgres:5432/piping",
            }
        except BaseException:
            # Capture this isolated server before teardown; never echo its log.
            with contextlib.suppress(OSError, ValueError, subprocess.SubprocessError):
                write(self.stage / (purpose + "-postgres.log"), run(compose + ["logs", "--no-color", "postgres"]))
            raise
        finally:
            # This exact generated Compose project owns every temporary resource.
            run(compose + ["down", "--volumes", "--timeout", "30"], timeout=90)
            # PostgreSQL owns files as its image UID. A networkless disposable
            # container removes only this operation's exact scratch child after
            # the database has stopped; no host credential or journal is mounted.
            run(["docker", "run", "--rm", "--pull", "never", "--network", "none", "--user", "0",
                 "--mount", "type=bind,src=" + str(scratch) + ",dst=/cleanup",
                 "--entrypoint", "sh", self.plan["postgres_image"], "-c", "rm -rf /cleanup/postgres"], timeout=120)
            scratch.rmdir()

    @contextlib.contextmanager
    def database_target(self, database):
        production_sql = self.sql
        previous_database = getattr(self, "_active_database", None)
        def isolated_sql(query, *, timeout=120):
            return run(database["compose"] + ["exec", "-T", "postgres", "psql", "-X", "-qAt",
                                              "-v", "ON_ERROR_STOP=1", "-v", "VERBOSITY=verbose",
                                              "-U", "piping", "-d", database.get("name", "piping")],
                       data=query.encode(), psql=True, timeout=timeout).strip()
        self.sql = isolated_sql
        self._active_database = database
        try:
            yield
        finally:
            self.sql = production_sql
            self._active_database = previous_database

    @contextlib.contextmanager
    def isolated_environment(self, database, *, start_mode=None):
        values = read_proof_environment(self.stage / "proof-backend.env")
        url = database["url"]
        require(isinstance(url, str) and not any(c in url for c in "\r\n\x00"),
                "unsafe isolated database URL")
        values["PIPING_DATABASE__URL"] = url
        if start_mode is not None:
            require(start_mode == "migrate", "unsupported isolated start mode")
            values.update({"PIPING_RUN_MODE": "backend", "PIPING_START_MODE": start_mode,
                           "PIPING_START_EMBEDDED_GUACD": "false"})
        path = self.stage / ("isolated-backend-" + uuid.uuid4().hex + ".env")
        write(path, "".join(key + "=" + value + "\n" for key, value in values.items()).encode())
        try:
            yield path
        finally:
            path.unlink()

    def isolated_migrate(self, database, image):
        require(image in (self.image("backend"), self.plan.get("installed_backend_image")),
                "unverified migration executable")
        name = "piping-migrator-" + uuid.uuid4().hex
        label = "piping.release-stage=" + self.stage.name
        try:
            with self.isolated_environment(database, start_mode="migrate") as private_environment:
                run(["docker", "run", "--name", name, "--label", label, "--pull", "never",
                     "--network", database["network"], "--cpus", "1", "--memory", "1g",
                     "--env-file", str(private_environment), image], timeout=300)
        finally:
            ids = run(["docker", "ps", "-aq", "--filter", "name=^/" + name + "$",
                       "--filter", "label=" + label]).decode().splitlines()
            require(len(ids) <= 1, "ambiguous isolated migrator ownership")
            if ids:
                run(["docker", "rm", "-f", ids[0]])

    def native_content_type_seed_keys(self, source_database):
        """Derive seed keys only by executing verified source on an empty schema clone."""
        installed = len(self.plan["installed_history"])
        manifest = self.plan["prefix_manifest_sha384"]
        content_type_seeds_pending(manifest, installed)
        pending = sorted((int(PurePosixPath(path).name.split("_", 1)[0]), path)
                         for path in manifest
                         if int(PurePosixPath(path).name.split("_", 1)[0]) > installed)
        if not pending:
            return []
        with tempfile.TemporaryFile(dir=self.stage) as archive:
            run(source_database["compose"] + ["exec", "-T", "postgres", "pg_dump", "-U", "piping",
                "-d", source_database.get("name", "piping"), "--schema-only", "-Fc", "--no-owner",
                "--no-privileges"], output=archive, timeout=300)
            archive.seek(0)
            with self.isolated_database("content-type-source") as clone:
                self.restore_archive(clone, archive)
                with self.database_target(clone):
                    columns = json.loads(self.sql(
                        "SELECT coalesce(json_agg(json_build_array(column_name,udt_name) ORDER BY ordinal_position),'[]'::json) "
                        "FROM information_schema.columns WHERE table_schema='public' "
                        "AND table_name='inventory_content_type';"))
                    require(columns == [["id", "uuid"], ["app_label", "varchar"], ["model", "varchar"]],
                            "unsupported content-type registry shape")

                    def registry():
                        rows = json.loads(self.sql(
                            "SELECT coalesce(json_agg(json_build_array(id::text,app_label,model) "
                            "ORDER BY id::text),'[]'::json) FROM public.inventory_content_type;"))
                        require(isinstance(rows, list) and len(rows) <= 100000
                                and all(isinstance(row, list) and len(row) == 3
                                and all(isinstance(value, str) for value in row)
                                and len(row[1]) <= 100 and len(row[2]) <= 100
                                and str(uuid.UUID(row[0])) == row[0] for row in rows),
                                "invalid content-type source registry")
                        result = {row[0]: (row[1], row[2]) for row in rows}
                        require(len(result) == len(rows) and len(set(result.values())) == len(rows),
                                "ambiguous content-type source registry")
                        return result

                    before = registry()
                    require(not before, "content-type source clone must start empty")
                    for version, path in pending:
                        content = self.source_migration(PurePosixPath(path).name)
                        self.sql("BEGIN;\n" + content.decode("utf-8") + "\nCOMMIT;")
                        after = registry()
                        require(all(after.get(identity) == key for identity, key in before.items()),
                                "source migration changed existing content-type identity")
                        require(all(uuid.UUID(identity).version == 7 for identity in after.keys() - before.keys()),
                                "source content-type additions require UUIDv7")
                        if version not in CONTENT_TYPE_SEED_MIGRATIONS:
                            require(after == before, "unapproved migration changed content-type registry")
                        before = after
                    return [list(key) for key in sorted(before.values())]

    def reference_proof(self, backend, *, existing_environment=False):
        environment_values = backend.get("environment", {})
        require(isinstance(environment_values, dict), "invalid backend environment")
        lines = []
        for key, value in environment_values.items():
            require(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key) and isinstance(value, str)
                    and not any(c in value for c in "\r\n\x00"), "unsafe backend environment")
            lines.append(key + "=" + value + "\n")
        if not existing_environment:
            write(self.stage / "proof-backend.env", "".join(lines).encode())
        installed_version = len(self.plan["installed_history"])
        pending = installed_version < len(self.plan["migrations"])
        if pending:
            old_migrations, _, _ = self.published_migrations(self.plan["installed_backend_image"])
            require(old_migrations == {k: v for k, v in self.plan["migrations"].items() if int(k) <= installed_version},
                    "installed backend does not own the canonical prefix")
        with self.isolated_database("source-canonical") as database:
            self.isolated_migrate(database, self.plan["installed_backend_image"] if pending else self.image("backend"))
            with self.database_target(database):
                source_catalog = self.catalog()
                source_slots = self.layout_slots()
                source_history = self.guard_history(preserve_installed=False)
                require(len(source_history) == installed_version, "source reference is not the installed prefix")
            require(self.plan.get("source_raw_slots", source_slots) == source_slots,
                    "canonical source physical layout changed")
            self.plan["source_raw_slots"] = source_slots
            seed_keys = self.native_content_type_seed_keys(database)
            require(self.plan.get("content_type_seed_keys", seed_keys) == seed_keys,
                    "native content-type source oracle changed")
            self.plan["content_type_seed_keys"] = seed_keys
            delta = {"additions": [], "replacements": [], "removals": []}
            if pending:
                with self.isolated_database("source-fresh") as fresh:
                    self.isolated_migrate(fresh, self.image("backend"))
                    with self.database_target(fresh):
                        fresh_catalog = self.catalog()
                        self.guard_history(completed=True, preserve_installed=False)
                    delta = forward_catalog_delta(source_catalog, fresh_catalog,
                        prefix_manifest=self.plan["prefix_manifest_sha384"], installed_prefix=installed_version)
            require(self.plan.get("source_delta", delta) == delta, "native source migration delta changed")
            self.plan["source_delta"] = delta
            if getattr(self, "verifier", False):
                # A copied dossier cannot observe live installation slots.
                # Keep the staged identity; the independent native schema
                # roundtrip below proves its restored layout separately, since
                # pg_dump compacts dropped attributes rather than retaining gaps.
                slots = self.plan["installed_append_slots"]
            else:
                slots = self.append_slots()
            if "native_source_proven" in self.plan:
                require(slots == self.plan["installed_append_slots"], "target append layout changed")
            self.plan["installed_append_slots"] = slots
            self.plan["native_source_proven"] = True
            restored_source, source_checks, restored_source_slots = self.source_roundtrip_checks(database, source_catalog)
            require(self.plan.get("source_roundtrip_checks", source_checks) == source_checks,
                    "native source CHECK roundtrip changed")
            self.plan["source_roundtrip_checks"] = source_checks
            self.source_readiness(source_catalog, restored_source, source_checks, database, source_slots, restored_source_slots)
            if pending:
                self.isolated_migrate(database, self.image("backend"))
                with self.database_target(database):
                    exact_target_preservation(source_catalog, self.catalog(), **delta)
                    source_candidate_slots = self.layout_slots()
                    candidate_history = self.guard_history(completed=True, preserve_installed=False)
                    require(candidate_history[:installed_version] == source_history,
                            "source migrator rewrote installed SQLx metadata")
            else:
                source_candidate_slots = source_slots
            require(self.plan.get("source_candidate_slots", source_candidate_slots) == source_candidate_slots,
                    "native candidate physical layout changed")
            self.plan["source_candidate_slots"] = source_candidate_slots
        # A separate target-native oracle starts from the actual private schema
        # snapshot, never from a factory catalog or a client diagnostic receipt.
        with self.isolated_database("installed-target-reference") as database:
            self.restore_archive(database, self.stage / "installed-target-schema.dump")
            with self.database_target(database):
                restored = self.roundtrip_target(database)
                restored_layout_slots = self.layout_slots()
                require(restored_layout_slots == compacted_layout_slots(restored),
                        "restored target has unproven terminal attributes")
                self.seed_reference_history()
            self.plan["restored_target_catalog"] = restored
            require(self.plan.get("restored_layout_slots", restored_layout_slots) == restored_layout_slots,
                    "restored target physical layout changed")
            self.plan["restored_layout_slots"] = restored_layout_slots
            additions = self.target_additions(restored_layout_slots, restored, source_slots)
            replacements, removals = self.target_layout_edges(restored, restored_layout_slots, source_slots)
            restored_candidate_slots = self.candidate_layout_slots(restored_layout_slots, source_slots, source_candidate_slots)
            self.isolated_migrate(database, self.image("backend"))
            with self.database_target(database):
                candidate = self.catalog()
                exact_target_preservation(restored, candidate, additions=additions,
                                          replacements=replacements, removals=removals,
                                          source_delta=self.pending_delta(),
                                          layout_contract=self.plan["contract"] if self.workspace_cutover() else None)
                require(self.layout_slots() == restored_candidate_slots,
                        "restored candidate physical layout differs from source transition")
                self.guard_history(completed=True)
            self.plan["restored_candidate_catalog"] = candidate
            self.plan["restored_candidate_slots"] = restored_candidate_slots
        self.plan["installed_candidate_slots"] = self.candidate_layout_slots(
            self.plan["installed_layout_slots"], source_slots, source_candidate_slots)
        self.plan["installed_candidate_additions"] = self.target_additions(
            self.plan["installed_layout_slots"], self.plan["installed_target_catalog"], source_slots)
        (self.plan["installed_candidate_replacements"], self.plan["installed_candidate_removals"]
         ) = self.target_layout_edges(self.plan["installed_target_catalog"], self.plan["installed_layout_slots"], source_slots)

    def source_roundtrip_checks(self, database, source_catalog):
        # This oracle authorizes only native reparse of source-owned CHECKs. It
        # never supplies the target baseline, extension definitions or row data.
        with self.database_target(database):
            self.sql("CREATE DATABASE source_check_roundtrip;")
        with tempfile.TemporaryFile(dir=self.stage) as archive:
            run(database["compose"] + ["exec", "-T", "postgres", "pg_dump", "-Fc", "--schema-only",
                                      "-U", "piping", "-d", "piping"], output=archive, timeout=300)
            archive.seek(0)
            target = {**database, "name": "source_check_roundtrip"}
            self.restore_archive(target, archive)
        with self.database_target(target):
            restored = self.catalog()
            slots = self.layout_slots()
            require(slots == compacted_layout_slots(restored), "source roundtrip did not compact all attribute slots")
        after = {(o["kind"], o["name"]): o["definition"] for o in restored}
        checks = {o["name"]: [o["definition"], after[(o["kind"], o["name"])]]
                  for o in source_catalog if o["kind"] == "constraint" and o["definition"].startswith("CHECK ")}
        expected = json.loads(json.dumps(source_catalog))
        tables = {}
        for obj in expected:
            if obj["kind"] == "column":
                tables.setdefault(obj["name"].rsplit(".", 1)[0], []).append(obj)
            elif obj["kind"] == "constraint" and obj["name"] in checks:
                obj["definition"] = checks[obj["name"]][1]
        for columns in tables.values():
            for position, obj in enumerate(sorted(columns, key=lambda o: o["definition"]["position"]), 1):
                obj["definition"]["position"] = position
        exact_target_preservation(expected, restored)
        return restored, checks, slots

    def restore_archive(self, database, archive):
        with contextlib.ExitStack() as stack:
            source = archive if hasattr(archive, "read") else stack.enter_context(checked_path(archive).open("rb"))
            output = stack.enter_context(tempfile.TemporaryFile())
            result = None
            try:
                result = subprocess.run(database["compose"] + ["exec", "-T", "postgres", "pg_restore",
                    "--exit-on-error", "--no-owner", "--no-privileges", "-U", "piping", "-d", database.get("name", "piping")],
                    stdin=source, stdout=output, stderr=output, timeout=POSTGRES_RESTORE_TIMEOUT_SECONDS,
                    env=environment(), check=False)
            finally:
                if result is None or result.returncode:
                    output.seek(0)
                    write(self.stage / ("restore-native-" + uuid.uuid4().hex + ".log"), output.read(MAX_BYTES))
            require(result.returncode == 0, "isolated native restore failed")

    def seed_reference_history(self):
        rows = json.dumps(self.plan["installed_history"], separators=(",", ":")).replace("'", "''")
        self.sql("INSERT INTO public._sqlx_migrations(version,description,installed_on,success,checksum,execution_time) "
                 "SELECT version::bigint,description,installed_on::timestamptz,success,decode(checksum,'hex'),execution_time "
                 "FROM json_to_recordset('" + rows + "'::json) AS r(version text,description text,installed_on text,"
                 "success boolean,checksum text,execution_time bigint);")


    def restore_proof(self, baseline):
        exact_target_preservation(self.plan["installed_target_catalog"], baseline["catalog"])
        conversion_semantics = None
        if os.path.lexists(self.stage / "original-r-manifest.json"):
            self.snapshot_manifest("original-r")
            source = read_json(self.stage / "original-r-source.json")
            original = read_json(self.stage / "original-r-fingerprint.json")
            with self.isolated_database("original-r-replay") as original_database:
                self.raw_restore(original_database, self.stage / "original-r.dump", original)
                with self.database_target(original_database):
                    self.validate_fhrp_source(source, restored=True)
                self.maintenance_run(original_database, convert=True)
                with self.database_target(original_database):
                    conversion_semantics = self.verify_fhrp_conversion(source, restored=True)
                    converted = self.fingerprint()
                self.maintenance_run(original_database, convert=True)
                with self.database_target(original_database):
                    require(self.fingerprint() == converted, "replayed prepared conversion changed state")
        with self.isolated_database("restore") as database:
            self.restore_archive(database, self.stage / "offline-pre-upgrade.dump")
            with self.database_target(database):
                restored = self.fingerprint()
                expected_restored = {**baseline, "catalog": self.plan["restored_target_catalog"]}
                if restored != expected_restored:
                    expected_objects = {(o["kind"], o["name"]): o for o in expected_restored["catalog"]}
                    actual_objects = {(o["kind"], o["name"]): o for o in restored["catalog"]}
                    write_json(self.stage / "restore-difference.json", {
                        "catalog": [{"identity": list(key), "expected": expected_objects.get(key), "actual": actual_objects.get(key)}
                                    for key in sorted(set(expected_objects) | set(actual_objects))
                                    if expected_objects.get(key) != actual_objects.get(key)],
                        "row_tables": sorted(name for name in set(baseline["rows"]) | set(restored["rows"])
                                             if baseline["rows"].get(name) != restored["rows"].get(name)),
                        "columns_equal": baseline["columns"] == restored["columns"],
                        "history_equal": baseline["history"] == restored["history"],
                    })
                require(restored == expected_restored, "isolated restore changed backup state")
                require(self.layout_slots() == self.plan["restored_layout_slots"],
                        "isolated restore changed the complete physical layout")
                self.guard_workspace_prepared(baseline)
                if conversion_semantics is not None:
                    require(self.verify_fhrp_conversion(source, restored=True) == conversion_semantics,
                            "R replay and sealed C conversion semantics differ")
                if self.transforms():
                    require(self.migration_expectation(baseline, restored=True)
                            == self.load_migration_expectation(baseline),
                            "restored source does not reproduce the sealed data expectation")
            self.isolated_migrate(database, self.image("backend"))
            with self.database_target(database):
                self.guard_database(completed=True)
                self.verify_conservation(baseline, restored=True)

    def fingerprint(self, columns=None, *, project_transforms=False, content_type_ids=None,
                    monitoring_normalized=False):
        catalog = self.catalog()
        transforms = self.transforms()
        dataflow_pending = "Dataflow0017" in transforms
        projecting_old_columns = columns is not None
        live_columns = json.loads(
            self.sql(
                "SELECT coalesce(json_object_agg(name,cols),'{}'::json) FROM ("
                "SELECT table_schema || '.' || table_name AS name,json_agg(column_name ORDER BY ordinal_position) AS cols "
                "FROM information_schema.columns WHERE table_schema NOT LIKE 'pg\\_%' ESCAPE '\\' "
                "AND table_schema <> 'information_schema' GROUP BY table_schema,table_name) c;"
            )
        )
        columns = live_columns if columns is None else columns
        if project_transforms:
            require(transforms and not projecting_old_columns,
                    "native projection requires the exact pending data transformation")
            columns = transformation_columns(columns, transforms)
        monitoring_pending = "Monitoring0020" in transforms
        require(not monitoring_normalized or (monitoring_pending and projecting_old_columns and not project_transforms),
                "monitoring normalization requires the sealed transformed inventory")
        monitoring_projection = monitoring_pending and (project_transforms or monitoring_normalized)
        monitoring_tables = set(migration_contract.MONITORING_NEW_TABLES)
        if monitoring_projection:
            require(all(columns.get(table) == list(migration_contract.MONITORING_COLUMNS[table])
                        for table in monitoring_tables), "monitoring transformed columns differ from source")
            if project_transforms:
                require(live_columns.get(migration_contract.MONITORING_LEGACY_TABLE)
                        == list(migration_contract.MONITORING_LEGACY_COLUMNS)
                        and not monitoring_tables.intersection(live_columns),
                        "monitoring source is not the legacy-only inventory")
                require(self.sql("SELECT NOT EXISTS (SELECT FROM public.monitoring_exporter_assignment a "
                                 "LEFT JOIN public.monitoring_exporter e ON e.id=a.exporter_id "
                                 "WHERE e.id IS NULL);") == b"t", "monitoring assignment exporter is missing")
            else:
                require(migration_contract.MONITORING_LEGACY_TABLE not in live_columns
                        and all(live_columns.get(table) == list(migration_contract.MONITORING_COLUMNS[table])
                                for table in monitoring_tables),
                        "monitoring target is not the transformed inventory")
        added_tables = {o["name"] for o in self.plan.get("source_delta", {}).get("additions", [])
                        if o["kind"] == "relation" and o["definition"]["kind"] in ("r", "p", "v", "f")}
        # A pre-DDL source cannot expose post-DDL relations or columns. Those
        # changed tables are supplied by their pinned native projection instead.
        absent = set()
        if dataflow_pending:
            absent.add(DATAFLOW_REMOVED_TABLE)
            if project_transforms:
                absent.update(DATAFLOW_CHANGED_TABLES)
        if project_transforms and monitoring_pending:
            absent.update(monitoring_tables)
        require(set(columns) - absent <= set(live_columns)
                and set(live_columns) - set(columns) <= added_tables | absent | (
                    {SERVICE_MAPPING_TABLE} if project_transforms and self.service_cutover() else set()) | (
                    {migration_contract.MONITORING_LEGACY_TABLE} if project_transforms and monitoring_pending else set()),
                "database table coverage changed outside native source delta")
        statements = []
        if monitoring_projection:
            statements.extend((table, monitoring_digest_query(query)) for table, query in
                              monitoring_projection_queries(before=project_transforms).items())
        for table, names in columns.items():
            if monitoring_projection and table in monitoring_tables:
                continue
            if (project_transforms and dataflow_pending and table in DATAFLOW_CHANGED_TABLES
                    and (table not in live_columns
                         or any(name not in live_columns[table] for name in names))):
                continue
            computed = {"port_mappings"} if project_transforms and self.service_cutover() and table in SERVICE_TABLES else set()
            schedule_projection = (project_transforms and "ScheduleOwner0021" in transforms
                                   and table == migration_contract.SCHEDULE_TABLE)
            if schedule_projection:
                require({"delegated_by_user_id", "service_principal_id", "enabled", "metadata"}
                        <= set(live_columns[table]), "schedule source columns are incomplete")
                computed.add("owner_user_id")
            require(set(names) - computed <= set(live_columns[table]), "old database column missing")
            identifier = ".".join('"' + part.replace('"', '""') + '"' for part in table.split("."))
            owner_active = ("EXISTS (SELECT 1 FROM public.identity_user AS owner "
                            "WHERE owner.id=source.delegated_by_user_id AND owner.status='active')")
            schedule_expressions = {
                "owner_user_id": 'source."delegated_by_user_id" AS "owner_user_id"',
                "enabled": '(CASE WHEN ' + owner_active + ' THEN source.enabled ELSE false END) AS "enabled"',
                "metadata": '(source.metadata || jsonb_build_object('
                            "'migrated_from_service_principal_id',source.service_principal_id::text) || "
                            '(CASE WHEN ' + owner_active + " THEN '{}'::jsonb ELSE jsonb_build_object("
                            "'owner_migration_paused',true,'owner_migration_pause_reason',"
                            "'owner_missing_or_inactive') END)) AS \"metadata\"",
            } if schedule_projection else {}
            fields = ",".join(
                schedule_expressions.get(name)
                or
                (dataflow_cutover_expression(table, name) if dataflow_pending else None)
                or (service_cutover_expression(table) if name in computed else None)
                or 'source."' + name.replace('"', '""') + '"' for name in names)
            query = (
                "SELECT count(*)::text || ':' || "
                "encode(sha256(convert_to(coalesce(string_agg(h,'' ORDER BY h),''),'UTF8')),'hex') FROM "
                "(SELECT encode(sha256(convert_to(to_jsonb(t)::text,'UTF8')),'hex') AS h FROM "
                "(SELECT " + fields + " FROM " + identifier + " AS source) t) hashed"
            )
            statements.append((table, query))
        if content_type_ids is not None:
            require(isinstance(content_type_ids, list) and len(content_type_ids) <= 100000
                    and all(isinstance(value, str) and str(uuid.UUID(value)) == value for value in content_type_ids)
                    and content_type_ids == sorted(set(content_type_ids))
                    and columns.get(CONTENT_TYPE_TABLE) == ["id", "app_label", "model"],
                    "invalid content-type fingerprint identities")
            identifiers = ",".join("'" + value + "'::uuid" for value in content_type_ids)
            selection = "id=ANY(ARRAY[" + identifiers + "]::uuid[])"
            statements.extend((
                ("__content_type_digest", "SELECT count(*)::text || ':' || "
                 "encode(sha256(convert_to(coalesce(string_agg(h,'' ORDER BY h),''),'UTF8')),'hex') FROM "
                 "(SELECT encode(sha256(convert_to(to_jsonb(t)::text,'UTF8')),'hex') AS h FROM "
                 "(SELECT source.\"id\",source.\"app_label\",source.\"model\" "
                 "FROM public.inventory_content_type AS source WHERE " + selection + ") t) hashed"),
                ("__content_type_original_keys", "SELECT coalesce(json_agg(json_build_array(app_label,model) "
                 "ORDER BY app_label,model),'[]'::json)::text FROM public.inventory_content_type WHERE " + selection),
                ("__content_type_all_keys", "SELECT coalesce(json_agg(json_build_array(app_label,model) "
                 "ORDER BY app_label,model),'[]'::json)::text FROM public.inventory_content_type"),
                ("__content_type_new_ids", "SELECT coalesce(json_agg(id::text ORDER BY id::text),'[]'::json)::text "
                 "FROM public.inventory_content_type WHERE NOT (" + selection + ")"),
            ))
        for obj in catalog:
            if obj["kind"] == "sequence":
                # New source-owned sequences have no old state to conserve.
                if projecting_old_columns and any(o["kind"] == "sequence" and o["name"] == obj["name"]
                                                   for o in self.pending_delta()["additions"]):
                    continue
                identifier = ".".join('"' + part.replace('"', '""') + '"' for part in obj["name"].split("."))
                statements.append(("sequence:" + obj["name"],
                                   "SELECT last_value::text || ':' || is_called::text FROM " + identifier))
        # One native server-side batch retains complete table coverage without a
        # subprocess per table. Only a transaction-local temporary receipt is written.
        def literal(value):
            return "'" + value.replace("'", "''") + "'"
        batch = (
            "BEGIN ISOLATION LEVEL REPEATABLE READ; SET LOCAL standard_conforming_strings=on; SET LOCAL TimeZone='UTC'; "
            "CREATE TEMP TABLE piping_row_receipt(name text PRIMARY KEY,fingerprint text) ON COMMIT DROP; "
            "DO $piping_receipt$ DECLARE digest text; BEGIN "
            + " ".join("EXECUTE " + literal(query) + " INTO digest; INSERT INTO piping_row_receipt VALUES ("
                       + literal(name) + ",digest);" for name, query in statements)
            + " END $piping_receipt$; SELECT coalesce(json_object_agg(name,fingerprint),'{}'::json) "
              "FROM piping_row_receipt; COMMIT;"
        )
        require("$piping_receipt$" not in "".join(name + query for name, query in statements),
                "unsafe database receipt identifier")
        # This atomic batch covers every table, not one ordinary SQL command.
        # Keep its full comparison intact under the enclosing action deadline.
        rows = json.loads(self.sql(batch, timeout=300))
        result = {"catalog": catalog, "columns": columns, "rows": rows, "history": self.history()}
        if content_type_ids is not None:
            result["content_type_snapshot"] = {
                "original_digest": rows.pop("__content_type_digest"),
                "original_keys": json.loads(rows.pop("__content_type_original_keys")),
                "all_keys": json.loads(rows.pop("__content_type_all_keys")),
                "new_ids": json.loads(rows.pop("__content_type_new_ids")),
            }
        return result

    def verify_upgrade(self, baseline):
        self.guard_database(completed=True)
        self.verify_conservation(baseline)

    def monitoring_relation_checks(self):
        # SQLx executes 0020 and inserts its ledger row in one transaction.
        # New timestamps are transaction timestamps, not legacy timestamps.
        checks = (
            ("monitoring identities or profile coverage are not bijective",
             "SELECT (SELECT count(*)=count(DISTINCT id) FROM public.monitoring_profile) "
             "AND (SELECT count(*)=count(DISTINCT id) FROM public.monitoring_profile_item) "
             "AND (SELECT count(*)=count(DISTINCT id) AND count(*)=count(DISTINCT profile_id) "
             "FROM public.monitoring_profile_assignment) "
             "AND NOT EXISTS (SELECT FROM public.monitoring_profile_assignment a "
             "LEFT JOIN public.monitoring_profile p ON p.id=a.profile_id WHERE p.id IS NULL) "
             "AND NOT EXISTS (SELECT FROM public.monitoring_profile_item i "
             "LEFT JOIN public.monitoring_profile p ON p.id=i.profile_id "
             "LEFT JOIN public.monitoring_exporter e ON e.id=i.exporter_id "
             "WHERE p.id IS NULL OR e.id IS NULL) "
             "AND NOT EXISTS (SELECT FROM public.monitoring_profile_assignment a "
             "WHERE (SELECT count(*) FROM public.monitoring_profile_item i WHERE i.profile_id=a.profile_id)<>1) "
             "AND NOT EXISTS (SELECT FROM public.monitoring_profile_assignment a "
             "JOIN public.monitoring_profile p ON p.id=a.profile_id WHERE p.slug IN "
             "('linux-baseline','docker-host','web-nginx','postgres','network-blackbox','k8s-node'));"),
            ("monitoring UUIDv7 identities or migration timestamps are invalid",
             "WITH migration AS (SELECT installed_on FROM public._sqlx_migrations "
             "WHERE version=20 AND success), bounds AS (SELECT installed_on AS started, "
             "coalesce((SELECT min(installed_on) FROM public._sqlx_migrations WHERE version>20),"
             "clock_timestamp()) AS finished FROM migration), generated AS ("
             "SELECT id,created_at,updated_at FROM public.monitoring_profile UNION ALL "
             "SELECT id,created_at,updated_at FROM public.monitoring_profile_item) "
             "SELECT (SELECT count(*)=1 FROM migration) AND NOT EXISTS ("
             "SELECT FROM generated g CROSS JOIN bounds b WHERE "
             "uuid_extract_version(g.id) IS DISTINCT FROM 7 OR uuid_extract_timestamp(g.id) IS NULL "
             "OR uuid_extract_timestamp(g.id)<b.started-interval '1 millisecond' "
             "OR uuid_extract_timestamp(g.id)>b.finished+interval '1 millisecond' "
             "OR g.created_at IS DISTINCT FROM b.started OR g.updated_at IS DISTINCT FROM b.started) "
             "AND NOT EXISTS (SELECT FROM public.monitoring_profile_assignment a CROSS JOIN bounds b "
             "WHERE a.created_at IS DISTINCT FROM b.started OR a.updated_at IS DISTINCT FROM b.started);"),
        )
        for message, query in checks:
            require(self.sql(query) == b"t", message)

    def verify_conservation(self, baseline, *, restored=False):
        expected = self.load_migration_expectation(baseline) if self.transforms() else baseline
        if self.workspace_cutover():
            expected = workspace_0016_conservation(expected, self.workspace_delta())
        content_types = expected.get("content_type_seeds")
        actual = self.fingerprint(expected["columns"], content_type_ids=(
            content_types["original_ids"] if content_types is not None else None),
            monitoring_normalized="Monitoring0020" in self.transforms())
        if "Monitoring0020" in self.transforms():
            self.monitoring_relation_checks()
        if restored:
            exact_target_preservation(self.plan["restored_candidate_catalog"], actual["catalog"])
            require(self.layout_slots() == self.plan["restored_candidate_slots"],
                    "restored migration changed the proven terminal layout")
        else:
            exact_target_preservation(self.plan["installed_target_catalog"], actual["catalog"],
                                      additions=self.plan["installed_candidate_additions"],
                                      replacements=self.plan["installed_candidate_replacements"],
                                      removals=self.plan["installed_candidate_removals"],
                                      source_delta=self.pending_delta(),
                                      layout_contract=self.plan["contract"] if self.workspace_cutover() else None)
            require(self.layout_slots() == self.plan["installed_candidate_slots"],
                    "migration changed the proven terminal layout")
            if self.plan["preservation_sha256"]:
                require(self.installed_check_bindings() == self.plan["installed_check_bindings"],
                        "migration changed original native CHECK bindings")
                if self.plan["check_capture_sha256"]:
                    self.guard_original_check_identity()
        old_rows = dict(expected["rows"])
        new_rows = dict(actual["rows"])
        # The ledger is checked as a precise append above, never globally ignored.
        old_rows.pop("public._sqlx_migrations")
        new_rows.pop("public._sqlx_migrations")
        if content_types is not None:
            snapshot = actual["content_type_snapshot"]
            old_keys = [tuple(key) for key in snapshot["original_keys"]]
            all_keys = [tuple(key) for key in snapshot["all_keys"]]
            new_ids = snapshot["new_ids"]
            required_keys = {tuple(key) for key in content_types["native_keys"]}
            require(snapshot["original_digest"] == content_types["original_digest"]
                    and len(old_keys) == len(content_types["original_ids"])
                    and len(set(old_keys)) == len(old_keys)
                    and len(set(all_keys)) == len(all_keys)
                    and set(all_keys) == set(old_keys) | required_keys
                    and len(new_ids) == len(set(new_ids)) == len(all_keys) - len(old_keys)
                    and all(str(uuid.UUID(value)) == value and uuid.UUID(value).version == 7 for value in new_ids),
                    "content-type registry identities or required keys changed")
            old_rows.pop(CONTENT_TYPE_TABLE)
            new_rows.pop(CONTENT_TYPE_TABLE)
        if new_rows != old_rows:
            mismatched = sorted(name for name in old_rows.keys() | new_rows.keys()
                                if old_rows.get(name) != new_rows.get(name))
            visible = [name for name in mismatched if name in DIAGNOSTIC_TABLES][:16]
            def row_count(value):
                match = re.fullmatch(r"([0-9]{1,10}):[0-9a-f]{64}", value) if isinstance(value, str) else None
                count = int(match[1]) if match else None
                return count if count is not None and count <= 1000000000 else None
            error = DeployError("migration changed existing rows or ciphertext")
            error.native_failure_context = validated_failure_context({
                "mismatch_tables": [{"table": name, "expected_count": row_count(old_rows.get(name)),
                                     "actual_count": row_count(new_rows.get(name))} for name in visible],
                "mismatch_tables_truncated": len(visible) != len(mismatched),
            })
            raise error
        require(actual["history"][:len(baseline["history"])] == baseline["history"],
                "migration rewrote old ledger metadata")
        if self.workspace_cutover():
            require(self.sql('SELECT NOT EXISTS (SELECT FROM public.playbook_project_revision) '
                             'AND NOT EXISTS (SELECT FROM public.playbook_run_console_chunk);') == b't',
                    'workspace migration populated newly created tables')
        if len(baseline["history"]) < len(self.plan["migrations"]):
            for obj in self.pending_delta()["additions"]:
                if obj["kind"] == "column" and obj["name"].rsplit(".", 1)[0] in baseline["columns"]:
                    if ("ScheduleOwner0021" in self.transforms()
                            and obj["name"] in migration_contract.SCHEDULE_OWNER_RENAMES):
                        # The renamed value is checked against every original schedule row.
                        continue
                    if self.service_cutover() and obj["name"] in {table + ".port_mappings" for table in SERVICE_TABLES}:
                        # These values are checked against every pre-upgrade row,
                        # not mistaken for a default-valued additive column.
                        continue
                    _, table, column = obj["name"].split(".")
                    require(self.sql('SELECT NOT EXISTS (SELECT 1 FROM public."' + table + '" WHERE "'
                                      + column + '" IS DISTINCT FROM ' + (obj["definition"]["default"] or "NULL") + ');') == b"t",
                            "old rows acquired non-default migration values")
        if "Dataflow0017" in self.transforms():
            self.dataflow_relation_checks()

    def running(self):
        return set(run(self.compose() + ["ps", "--services", "--status", "running"]).decode().splitlines())

    def declared_services(self):
        return set(run(self.compose() + ["config", "--services"]).decode().splitlines())

    def running_declared_services(self):
        return sorted(self.running() & self.declared_services())

    def stopped(self):
        require(
            not set(WRITERS).intersection(self.running()),
            "application writers are still running",
        )

    def raw_archive(self, name, fingerprint_name):
        """Capture R or C without projecting, converting or changing its ledger."""
        require(all(isinstance(item, str) and Path(item).name == item for item in (name, fingerprint_name)),
                "unsafe raw archive name")
        self.stopped()
        self.guard_writer_ids()
        self.guard_config()
        self.guard_database()
        self.guard_target_catalog()
        if os.path.lexists(self.stage / "access-fence.json"):
            self.guard_access_fence()
        before = self.fingerprint()
        archive = self.stage / name
        checked_path(archive, missing=True)
        with archive.open("xb") as stream:
            os.chmod(archive, 0o600)
            run(self.postgres_command("sh", "-c",
                'exec pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" --format=custom'), output=stream, timeout=300)
            stream.flush()
            os.fsync(stream.fileno())
        require(archive.stat().st_size > 0, "fresh dump is empty")
        with archive.open("rb") as stream, tempfile.TemporaryFile() as captured:
            result = subprocess.run(self.postgres_command("pg_restore", "--list"), stdin=stream,
                                    stdout=captured, stderr=captured, timeout=120, env=environment(), check=False)
        require(result.returncode == 0, "fresh archive validation failed")
        require(self.fingerprint() == before, "database changed during fresh backup")
        self.stopped()
        self.guard_writer_ids()
        self.guard_config()
        self.guard_database()
        if os.path.lexists(self.stage / "access-fence.json"):
            self.guard_access_fence()
        write_json(self.stage / fingerprint_name, before)
        return before

    def raw_restore(self, database, archive, expected):
        """Restore into an empty target and compare against a source-derived layout."""
        require(expected["catalog"] == self.plan["installed_target_catalog"]
                and expected["history"] == self.plan["installed_history"],
                "raw backup differs from the staged source catalog or ledger")
        with self.database_target(database):
            require(self.sql("SELECT NOT EXISTS (SELECT FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace "
                             "WHERE n.nspname NOT LIKE 'pg\\_%' ESCAPE '\\' AND n.nspname<>'information_schema');") == b"t",
                    "raw restore target is not empty")
        self.restore_archive(database, archive)
        with self.database_target(database):
            require(self.fingerprint() == {**expected, "catalog": self.plan["restored_target_catalog"]},
                    "isolated restore changed original backup state")
            require(self.layout_slots() == self.plan["restored_layout_slots"],
                    "isolated restore changed original physical layout")

    def container_inputs(self, container, *, converter):
        """Read the actual packaged loader inputs; never infer a default cipher."""
        files = {}
        archive = run(["docker", "cp", container + ":/app/config", "-"])
        with tarfile.open(fileobj=io.BytesIO(archive)) as source:
            for item in source.getmembers():
                require(item.isdir() or item.isfile(), "unsupported settings archive member")
                if item.isdir():
                    continue
                require(item.name in ("config/default.toml", "config/local.toml", "config/local.toml.example")
                        and item.size <= 1024 * 1024, "unsupported settings input")
                require(item.name not in files, "duplicate settings input")
                files[item.name] = source.extractfile(item).read()
        require("config/default.toml" in files, "packaged default settings missing")
        defaults = tomllib.loads(files["config/default.toml"].decode())
        default_settings = {"canonical_sha256": sha(canonical(defaults)),
                            "retired_catalog_present": "official_catalog" in defaults,
                            "retained_sha256": sha(canonical({name: value for name, value in defaults.items()
                                                              if name != "official_catalog"}))}
        cipher_ref = None
        for name in ("config/default.toml", "config/local.toml"):
            if name in files:
                values = tomllib.loads(files[name].decode())
                cipher_ref = values.get("secrets", {}).get("cipher", {}).get("key_material_ref", cipher_ref)
        digests = {}
        for executable in ("piping-backend", *(("convert-fhrp-auth-keys",) if converter else ())):
            with tempfile.TemporaryFile(dir=self.stage) as stream:
                run(["docker", "cp", container + ":/usr/local/bin/" + executable, "-"], output=stream, timeout=300)
                stream.seek(0)
                with tarfile.open(fileobj=stream) as source:
                    members = source.getmembers()
                    require(len(members) == 1 and members[0].isfile()
                            and members[0].name == executable and 0 < members[0].size < 512 * 1024 * 1024
                            and members[0].mode & 0o111, "invalid packaged executable")
                    digests[executable] = hashlib.file_digest(source.extractfile(members[0]), "sha256").hexdigest()
        return {"settings": {name: sha(raw) for name, raw in files.items()},
                "default_settings": default_settings, "cipher_reference": cipher_ref, "executables": digests}

    def image_inputs(self, image, *, converter):
        container = run(["docker", "create", "--label", "piping.release-stage=" + self.stage.name,
                         "--entrypoint", "/bin/true", image]).decode().strip()
        try:
            require(re.fullmatch(r"[0-9a-f]{64}", container), "invalid input-inspection container")
            return self.container_inputs(container, converter=converter)
        finally:
            run(["docker", "rm", container])

    def packaged_settings_preserved(self, installed, candidate):
        if installed["cipher_reference"] != candidate["cipher_reference"]:
            return False
        if installed["settings"] == candidate["settings"]:
            return True
        if ({name: digest for name, digest in installed["settings"].items() if name != "config/default.toml"}
                != {name: digest for name, digest in candidate["settings"].items() if name != "config/default.toml"}):
            return False
        old, new = installed["default_settings"], candidate["default_settings"]
        # PIP-584 adds only relay availability and a default-false browser opt-in.
        # These complete source-TOML hashes bind every existing setting as well.
        if (old["canonical_sha256"], new["canonical_sha256"]) == (
            "64b3b5eb73044ea2ff697f0df4ab97dd6f7f81767cf5ffa0c05412cb104ba1b1",
            "c5c2f258ba8fa1d4f7e427a6131ac311437b7615c8c4c1788155fb34c9d6a0e3",
        ):
            return True
        if not self.workspace_cutover():
            return False
        # The closed workspace retirement also removes the unused stock catalog
        # defaults. Local overrides and every retained default remain exact.
        return (old["retired_catalog_present"] is True and new["retired_catalog_present"] is False
                and old["retained_sha256"] == new["canonical_sha256"])

    def guard_runner_runtime(self, model, inspected):
        """The recovery declaration must describe the retained runner, not a future recreate."""
        runner = model["services"]["ansible-runner"]
        image = json.loads(run(["docker", "image", "inspect", runner["image"]]))[0]
        require(inspected["Image"] == image["Id"], "installed runner image differs from its original Compose declaration")
        declared_hash = run(self.compose() + ["config", "--hash", "ansible-runner"]).decode().split()
        labels = inspected["Config"].get("Labels") or {}
        require(len(declared_hash) == 2 and declared_hash[0] == "ansible-runner"
                and re.fullmatch(r"[0-9a-f]{64}", declared_hash[1])
                and labels.get("com.docker.compose.config-hash") == declared_hash[1],
                "installed runner differs from the sealed original Compose service")
        defaults, config, host = image["Config"], inspected["Config"], inspected["HostConfig"]

        def env_map(values):
            require(all(isinstance(value, str) and "=" in value for value in values), "unsupported runner image environment")
            result = dict(value.split("=", 1) for value in values)
            require(len(result) == len(values), "ambiguous runner environment")
            return result

        expected_env = env_map(defaults.get("Env") or [])
        overrides = runner.get("environment") or {}
        require(all(isinstance(value, str) for value in overrides.values()), "runner environment must be explicitly resolved")
        expected_env.update(overrides)
        require(env_map(config.get("Env") or []) == expected_env, "installed runner environment differs from recovery inputs")
        for field, native in (("user", "User"), ("working_dir", "WorkingDir")):
            require((config.get(native) or "") == (runner.get(field) or defaults.get(native) or ""),
                    "installed runner user or working directory differs from recovery inputs")
        entrypoint = runner.get("entrypoint")
        command = runner.get("command")
        expected_entrypoint = defaults.get("Entrypoint") if entrypoint is None else entrypoint
        expected_command = (defaults.get("Cmd") if entrypoint is None else []) if command is None else command
        require((config.get("Entrypoint") or []) == (expected_entrypoint or [])
                and (config.get("Cmd") or []) == (expected_command or []),
                "installed runner command differs from recovery inputs")
        expected_labels = {**(defaults.get("Labels") or {}), **(runner.get("labels") or {})}
        customer_labels = lambda values: {key: value for key, value in values.items()
                                          if not key.startswith("com.docker.compose.")}
        require(customer_labels(labels) == customer_labels(expected_labels),
                "installed runner labels differ from the original startup/configuration inputs")
        for field, native in (("read_only", "ReadonlyRootfs"), ("privileged", "Privileged"), ("init", "Init")):
            require(bool(host.get(native)) == bool(runner.get(field, False)), "installed runner isolation settings differ")
        for field, native in (("group_add", "GroupAdd"), ("cap_add", "CapAdd"), ("cap_drop", "CapDrop"), ("security_opt", "SecurityOpt")):
            require(set(host.get(native) or []) == set(runner.get(field) or []), "installed runner security settings differ")
        policy, separator, retries = (runner.get("restart") or "no").partition(":")
        require(not separator or (policy == "on-failure" and retries.isdecimal()), "unsupported runner restart policy")
        require(host["RestartPolicy"] == {"Name": policy, "MaximumRetryCount": int(retries) if separator else 0},
                "installed runner restart policy differs from recovery inputs")
        tmpfs = {}
        for item in runner.get("tmpfs") or []:
            target, _, options = item.partition(":")
            require(target not in tmpfs, "ambiguous runner tmpfs declaration")
            tmpfs[target] = options
        require((host.get("Tmpfs") or {}) == tmpfs, "installed runner tmpfs differs from recovery inputs")

        expected_mounts = {}
        for mount in runner.get("volumes") or []:
            kind, target = mount["type"], mount["target"]
            require(target not in expected_mounts and kind in ("bind", "volume")
                    and set(mount) <= {"type", "source", "target", "read_only", "bind", "volume", "consistency"},
                    "unsupported runner recovery mount")
            if kind == "bind":
                options = mount.get("bind") or {}
                require(set(options) <= {"create_host_path", "propagation"}, "unsupported runner bind options")
                source, propagation = mount["source"], options.get("propagation", "rprivate")
            else:
                require(mount.get("source") in model.get("volumes", {})
                        and set(mount.get("volume") or {}) <= {"nocopy"}, "runner recovery needs an explicit named volume")
                volume = model["volumes"][mount["source"]]
                require(volume.get("driver", "local") == "local" and not volume.get("driver_opts"),
                        "unsupported runner volume provider")
                source, propagation = volume["name"], ""
            expected_mounts[target] = (kind, source, not mount.get("read_only", False), propagation)
        require(set(defaults.get("Volumes") or {}) <= set(expected_mounts) | set(tmpfs),
                "runner image declares an unrepresented anonymous data volume")
        actual_mounts = {}
        for mount in inspected.get("Mounts") or []:
            target, kind = mount["Destination"], mount["Type"]
            require(target not in actual_mounts and kind in ("bind", "volume"), "unsupported installed runner mount")
            actual_mounts[target] = (kind, mount["Source"] if kind == "bind" else mount["Name"],
                                     mount["RW"], mount.get("Propagation", ""))
        require(actual_mounts == expected_mounts, "installed runner mounts differ from recovery inputs")
        mode = runner.get("network_mode")
        if mode:
            require(mode in ("host", "none") and host["NetworkMode"] == mode, "unsupported or changed runner network mode")
        else:
            networks = {model["networks"][name]["name"] for name in runner.get("networks", {})}
            require(set(inspected["NetworkSettings"]["Networks"]) == networks, "installed runner networks differ from recovery inputs")

    def observed_writer_inputs(self):
        """Bind a PID1 observation to containers, immutable inputs and start epochs."""
        model = json.loads(run(self.compose() + ["config", "--format", "json"]))
        require(sha(canonical(model)) == self.plan["original_model_sha256"], "original Compose model changed before runtime observation")
        result = {}
        for role, identifier in self.plan["writer_ids"].items():
            inspected = json.loads(run(["docker", "inspect", identifier]))[0]
            require(inspected["Id"] == identifier
                    and inspected["Config"].get("Labels", {}).get("com.docker.compose.project") == self.args.project_name
                    and inspected["Config"].get("Labels", {}).get("com.docker.compose.service") == role,
                    "runtime observation writer ownership changed")
            if role == "ansible-runner":
                self.guard_runner_runtime(model, inspected)
            result[role] = {"container": identifier, "image": inspected["Image"],
                "inputs_sha256": sha(canonical({key: inspected[key] for key in ("Config", "HostConfig", "Mounts")})),
                "started_at": inspected["State"]["StartedAt"], "restart_count": inspected["RestartCount"]}
        return result

    def runtime_observation(self, *, running):
        observation = {"plan_sha256": file_sha(self.stage / "plan.json"), "writers": self.observed_writer_inputs()}
        if not running:
            require(read_json(self.stage / "runtime-inputs.json").get("pid1_observation") == observation,
                    "pre-stop PID1 authority is stale or belongs to different writer inputs")
        return observation

    def runtime_inputs(self):
        """Only the stock local provider, packaged TOML and explicit env material."""
        self.guard_config()
        self.guard_writer_ids()
        model = json.loads(run(self.compose() + ["config", "--format", "json"]))
        backend = model["services"]["backend"]
        self.guard_migration_consumer(backend)
        require(not backend.get("volumes") and not backend.get("configs") and not backend.get("secrets")
                and backend.get("working_dir", "/app") == "/app", "unsupported backend settings mounts or directory")
        expected_environment = read_proof_environment(self.stage / "proof-backend.env")
        require(backend.get("environment", {}) == expected_environment
                and all(key == key.upper() for key in expected_environment if key.startswith("PIPING_")),
                "effective backend settings differ from the staged environment")
        inspected = json.loads(run(["docker", "inspect", self.plan["writer_ids"]["backend"]]))[0]
        if "proxy" in self.plan["writer_ids"]:
            proxy = json.loads(run(["docker", "inspect", self.plan["writer_ids"]["proxy"]]))[0]
            require(proxy["Image"] == self.plan["target_backend_image"],
                    "skewed installed proxy/backend images have no exact shared recovery artifact")
        require(inspected["Image"] == self.plan["target_backend_image"]
                and all(m["Type"] == "tmpfs" and m["Destination"] == "/tmp/guac-drives" for m in inspected.get("Mounts", ()))
                and inspected["Config"]["WorkingDir"] == "/app", "installed backend settings provider changed")
        actual_environment = dict(item.split("=", 1) for item in inspected["Config"]["Env"])
        require(all(actual_environment.get(key) == value for key, value in expected_environment.items())
                and all(key in expected_environment for key in actual_environment if key.startswith("PIPING_")),
                "installed backend environment is not fully represented")
        old = self.image_inputs(self.plan["target_backend_image"], converter=False)
        candidate = self.image_inputs(self.image("backend"), converter=True)
        live = self.container_inputs(self.plan["writer_ids"]["backend"], converter=False)
        old_config = json.loads(run(["docker", "image", "inspect", self.plan["target_backend_image"]]))[0]["Config"]
        require(all(inspected["Config"].get(native) == (backend[field] if backend.get(field) is not None else old_config.get(native))
                    for field, native in (("entrypoint", "Entrypoint"), ("command", "Cmd"))),
                "installed backend command differs from its declared image/configuration")
        require(live == old and self.packaged_settings_preserved(old, candidate),
                "installed/candidate packaged settings differ or installed files were modified")
        cipher_ref = expected_environment.get("PIPING_SECRETS__CIPHER__KEY_MATERIAL_REF", candidate["cipher_reference"])
        require(isinstance(cipher_ref, str) and re.fullmatch(r"env:[A-Z_][A-Z_0-9]*", cipher_ref),
                "unsupported backend cipher material provider")
        key = cipher_ref[4:]
        require(key in expected_environment and actual_environment.get(key) == expected_environment[key],
                "cipher environment material is not explicitly sealed")
        try:
            valid_cipher = len(base64.b64decode(expected_environment[key], validate=True)) == 32
        except (ValueError, TypeError):
            valid_cipher = False
        require(valid_cipher, "invalid supported cipher environment material")
        effective_hash = sha(canonical({"reference": cipher_ref, "material": expected_environment[key],
                                        "database_url": expected_environment["PIPING_DATABASE__URL"]}))
        if inspected["State"]["Running"]:
            # Docker Config.Env is not enough: an entrypoint may export a
            # different provider/material before exec. Observe PID1's actual
            # cipher/target environment without exposing or persisting it again.
            raw = run(["docker", "exec", inspected["Id"], "cat", "/proc/1/environ"])
            process = dict(item.split(b"=", 1) for item in raw.split(b"\x00") if item)
            process_ref = process.get(b"PIPING_SECRETS__CIPHER__KEY_MATERIAL_REF",
                                      (old["cipher_reference"] or "").encode())
            require(process_ref == cipher_ref.encode() and process.get(key.encode()) == expected_environment[key].encode()
                    and process.get(b"PIPING_DATABASE__URL") == expected_environment["PIPING_DATABASE__URL"].encode(),
                    "actual backend cipher/target environment differs from the sealed settings")
        else:
            require(os.path.lexists(self.stage / "runtime-inputs.json"),
                     "effective backend environment was not observed before stopping")
        observation = self.runtime_observation(running=inspected["State"]["Running"])
        return {"schema": "piping.deployment-runtime-inputs/v1", "installed": old, "candidate": candidate,
                 "environment_sha256": file_sha(self.stage / "proof-backend.env"),
                 "effective_cipher_reference": cipher_ref, "effective_cipher_environment_sha256": effective_hash,
                 "pid1_observation": observation}

    def guard_runtime_inputs(self):
        require(self.runtime_inputs() == read_json(self.stage / "runtime-inputs.json"),
                "sealed artifact pair or effective settings changed")

    def maintenance_run(self, database, *, convert=False):
        """One paired executable path, private env, no mounts or registry lookup."""
        image = self.image("backend")
        require(self.image_inputs(image, converter=True) == read_json(self.stage / "runtime-inputs.json")["candidate"],
                "paired executable or packaged configuration changed")
        name = "piping-maintenance-client-" + uuid.uuid4().hex
        label = "piping.release-stage=" + self.stage.name
        executable = "convert-fhrp-auth-keys" if convert else "piping-backend"
        try:
            with self.isolated_environment(database, start_mode=None if convert else "migrate") as private:
                run(["docker", "run", "--name", name, "--label", label,
                     *(["--label", "piping.fence=" + database["fence_nonce"]] if "fence_nonce" in database else []), "--pull", "never",
                     "--network", database["network"], "--cpus", "1", "--memory", "1g", "--env-file", str(private),
                     "--workdir", "/app", "--entrypoint", "/usr/local/bin/" + executable, image,
                     *(("--offline-confirmed", "--backup-confirmed", "--convert-fhrp-auth-keys") if convert else ())], timeout=300)
        finally:
            ids = run(["docker", "ps", "-aq", "--filter", "name=^/" + name + "$", "--filter", "label=" + label]).decode().splitlines()
            require(len(ids) <= 1, "ambiguous owned maintenance client")
            if ids:
                run(["docker", "rm", "-f", ids[0]])

    def fenced_target(self):
        fence = self.guard_access_fence()
        values = read_proof_environment(self.stage / "proof-backend.env")
        return {"compose": self.compose(), "name": "piping", "network": fence["name"],
                "url": values["PIPING_DATABASE__URL"], "fence_nonce": fence["nonce"]}

    def secret_graph(self):
        return {table: json.loads(self.sql("BEGIN; SET LOCAL TimeZone='UTC'; "
                                          "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),"
                                          "'[]'::jsonb) FROM public." + table + " t; COMMIT;"))
                for table in ("secrets_variable", "secrets_store", "secrets_binding", "secret_usage_binding")}

    def fhrp_source_state(self):
        columns = self.fingerprint()["columns"]
        require(FHRP_TABLE in columns and "auth_key" in columns[FHRP_TABLE], "original FHRP column missing")
        columns[FHRP_TABLE].remove("auth_key")
        return {"graph": self.secret_graph(), "non_key": self.fingerprint(columns),
                "pending": json.loads(self.sql("SELECT coalesce(jsonb_agg(id ORDER BY id),'[]'::jsonb) "
                                               "FROM ipam_fhrpgroup WHERE auth_key<>'';"))}

    def verify_fhrp_conversion(self, source, *, restored=False):
        require(self.sql("SELECT NOT EXISTS (SELECT FROM ipam_fhrpgroup WHERE auth_key IS DISTINCT FROM '');") == b"t",
                "canonical conversion did not produce prepared state")
        actual = self.fingerprint(source["non_key"]["columns"])
        require(actual["catalog"] == (self.plan["restored_target_catalog"] if restored else source["non_key"]["catalog"])
                and actual["history"] == source["non_key"]["history"], "conversion changed source schema or ledger")
        graph = self.secret_graph()
        added = {}
        for table, old in source["graph"].items():
            require(all(row in graph[table] for row in old), "conversion changed existing secret graph or ciphertext")
            added[table] = [row for row in graph[table] if row not in old]
            require(len(added[table]) == len(source["pending"]), "unattributed conversion graph additions")
        untouched = set(source["non_key"]["rows"]) - {"public." + table for table in graph}
        require(all(actual["rows"][table] == source["non_key"]["rows"][table] for table in untouched),
                "conversion changed non-key fields, unrelated rows, sequences or ledger")
        semantics, secret_ids = [], set()
        for group in source["pending"]:
            usage = [row for row in added["secret_usage_binding"] if row["consumer_id"] == group]
            require(len(usage) == 1, "converted group has ambiguous usage")
            usage = usage[0]
            require(usage["consumer_type"] == usage["owner_scope_type"] == "inventory_fhrp_group"
                    and usage["consumer_id"] == usage["owner_scope_id"] == group and usage["field_key"] == "auth_key",
                    "converted group usage/owner identity mismatch")
            secret = usage["secret_id"]
            require(secret not in secret_ids, "converted groups unexpectedly share a secret")
            secret_ids.add(secret)
            variables = [row for row in added["secrets_variable"] if row["id"] == secret]
            bindings = [row for row in added["secrets_binding"] if row["secret_id"] == secret]
            stores = [row for row in added["secrets_store"] if row["secret_id"] == secret]
            require(len(variables) == len(bindings) == len(stores) == 1, "incomplete conversion graph correspondence")
            variable, binding = variables[0], bindings[0]
            require(variable["kind"] == "field" and variable["retire_with_field_usage"] is True
                    and variable["is_masked"] is True and variable["is_protected"] is False
                    and variable["key"] == f"secret-field/{secret}/inventory_fhrp_group/{group}/{group}/auth_key"
                    and binding["scope_type"] == "inventory_fhrp_group" and binding["scope_id"] == group
                    and binding["managed_by_field_usage"] is True and binding["weight"] == 0
                    and isinstance(stores[0]["encrypted_value"], str) and stores[0]["encrypted_value"],
                    "noncanonical conversion variable, owner binding or encrypted store")
            semantics.append({key: usage[key] for key in ("consumer_type", "consumer_id", "field_key", "owner_scope_type", "owner_scope_id")})
        require({row["id"] for row in added["secrets_variable"]} == secret_ids
                == {row["secret_id"] for row in added["secrets_binding"]}
                == {row["secret_id"] for row in added["secrets_store"]}, "conversion graph contains unrelated additions")
        return semantics

    def validate_fhrp_source(self, source, *, restored=False):
        observed = self.fhrp_source_state()
        expected = {**source, "non_key": {**source["non_key"], "catalog": (
            self.plan["restored_target_catalog"] if restored else source["non_key"]["catalog"])}}
        require(observed == expected, "original R graph, pending groups or non-key state differ from the sealed source")

    def prove_original_r_request(self):
        """Independent signer phase: authenticate no target mutation beforehand."""
        self.snapshot_manifest("original-r", authenticated=False)
        source = read_json(self.stage / "original-r-source.json")
        require(isinstance(source["pending"], list) and source["pending"], "original R authorization has no pending groups")
        original = read_json(self.stage / "original-r-fingerprint.json")
        with self.isolated_database("original-r-authorization") as database:
            self.raw_restore(database, self.stage / "original-r.dump", original)
            with self.database_target(database):
                self.validate_fhrp_source(source, restored=True)
            self.maintenance_run(database, convert=True)
            with self.database_target(database):
                semantics = self.verify_fhrp_conversion(source, restored=True)
                prepared = self.fingerprint()
                write_json(self.stage / "migration-expectation.json", self.migration_expectation(prepared, restored=True))
            self.maintenance_run(database, convert=True)
            with self.database_target(database):
                require(self.fingerprint() == prepared, "R authorization replay changed prepared state")
            self.isolated_migrate(database, self.image("backend"))
            with self.database_target(database):
                self.guard_database(completed=True)
                self.verify_conservation(prepared, restored=True)
            return semantics

    def database_writer_guard(self):
        """The supported local PostgreSQL provider has no unattended SQL writers."""
        require(self.sql("SELECT current_user='piping' AND current_database()='piping' "
                         "AND current_setting('server_version_num')::int/10000=18 "
                         "AND current_setting('port')='5432' "
                         "AND (SELECT rolsuper FROM pg_roles WHERE rolname=current_user) "
                         "AND NOT EXISTS (SELECT FROM pg_stat_activity WHERE backend_type='client backend' "
                         "AND pid<>pg_backend_pid()) AND NOT EXISTS (SELECT FROM pg_prepared_xacts) "
                         "AND NOT EXISTS (SELECT FROM pg_subscription) AND NOT EXISTS (SELECT FROM pg_replication_slots) "
                         "AND NOT EXISTS (SELECT FROM pg_stat_replication) "
                         "AND NOT EXISTS (SELECT FROM pg_extension WHERE extname NOT IN ('plpgsql','pgcrypto')) "
                         "AND current_setting('shared_preload_libraries')='' "
                         "AND current_setting('session_preload_libraries')='' AND current_setting('local_preload_libraries')='' "
                         "AND NOT EXISTS (SELECT FROM pg_tablespace WHERE pg_tablespace_location(oid)<>'');") == b"t",
                "unsupported PostgreSQL provider or active/unattended database writers")

    def postgres_identity(self, container=None):
        ids = [container] if container is not None else run(self.compose() + ["ps", "-q", "postgres"]).decode().splitlines()
        require(len(ids) == 1, "ambiguous PostgreSQL container")
        inspected = json.loads(run(["docker", "inspect", ids[0]]))[0]
        require(inspected["State"]["Running"] and inspected["Image"] == self.plan["postgres_image"]
                and inspected["Config"]["Labels"].get("com.docker.compose.project") == self.args.project_name
                and inspected["Config"]["Labels"].get("com.docker.compose.service") == "postgres",
                "PostgreSQL container/image ownership mismatch")
        values = dict(item.split("=", 1) for item in inspected["Config"].get("Env", ()))
        # PG_SHA256 is stock image build metadata, not a libpq client setting.
        invalid_keys = {key for key in values if key.startswith("PG")} - {"PGDATA", "PG_MAJOR", "PG_VERSION", "PG_SHA256"}
        user_matches = values.get("POSTGRES_USER", "piping") == "piping"
        database_matches = values.get("POSTGRES_DB", "piping") == "piping"
        password_present = bool(values.get("POSTGRES_PASSWORD"))
        file_key_present = any(key.endswith("_FILE") for key in values if key.startswith("POSTGRES_"))
        if invalid_keys or not user_matches or not database_matches or not password_present or file_key_present:
            visible = sorted(key for key in invalid_keys if re.fullmatch(r"PG[A-Z0-9_]{0,62}", key))[:16]
            error = DeployError("unsupported PostgreSQL client environment or password provider")
            error.native_failure_context = validated_failure_context({
                "pg_invalid_keys": visible, "pg_invalid_keys_truncated": len(visible) != len(invalid_keys),
                "pg_user_matches": user_matches, "pg_database_matches": database_matches,
                "pg_password_present": password_present, "pg_file_key_present": file_key_present,
            })
            raise error
        server = json.loads(self.pinned_postgres_sql(ids[0], "SELECT json_build_object('system_identifier',system_identifier::text,"
                                     "'data_directory',current_setting('data_directory'),"
                                     "'hba_file',current_setting('hba_file')) FROM pg_control_system();"))
        data, hba = PurePosixPath(server["data_directory"]), PurePosixPath(server["hba_file"])
        require(data.is_absolute() and hba.is_absolute() and ".." not in data.parts + hba.parts
                and data in hba.parents and hba.name == "pg_hba.conf", "unsupported PostgreSQL HBA layout")
        mounts = inspected["Mounts"]
        require(len(mounts) == 1 and any(PurePosixPath(m["Destination"]) == data or PurePosixPath(m["Destination"]) in data.parents
                    for m in mounts if m.get("RW") is True and m["Type"] in ("volume", "bind")),
                "PostgreSQL has no retained identified data storage")
        return {"container": ids[0], "image": inspected["Image"], "mounts": mounts, **server}

    def pinned_postgres_sql(self, container, query):
        require(isinstance(container, str) and re.fullmatch(r"[0-9a-f]{64}", container), "invalid pinned PostgreSQL identity")
        return run(["docker", "exec", "-i", container, "psql", "-X", "-qAt", "-v", "ON_ERROR_STOP=1",
                    "-h", "/var/run/postgresql", "-p", "5432", "-U", "piping", "-d", "piping"], data=query.encode()).strip()

    def hba_bytes(self, identity):
        return run(["docker", "exec", "--user", "postgres", identity["container"], "sh", "-ceu",
                    'test -f "$1" && test ! -L "$1"; exec cat "$1"', "hba-read", identity["hba_file"]])

    def replace_hba(self, identity, content, nonce):
        require(re.fullmatch(r"[0-9a-f]{32}", nonce), "invalid fence operation identity")
        run(["docker", "exec", "-i", "--user", "postgres", identity["container"], "sh", "-ceu",
             'test -f "$1" && test ! -L "$1"; umask 077; tmp="$1.piping-$2"; '
             '(set -C; cat > "$tmp"); chmod 600 "$tmp"; mv -T "$tmp" "$1"',
             "hba-write", identity["hba_file"], nonce], data=content)
        require(self.pinned_postgres_sql(identity["container"], "SELECT pg_reload_conf();") == b"t", "native access fence reload failed")
        require(self.pinned_postgres_sql(identity["container"], "SELECT NOT EXISTS (SELECT FROM pg_hba_file_rules WHERE error IS NOT NULL);") == b"t",
                "native access fence has invalid rules")

    @staticmethod
    def private_network(value, *, prefix=None):
        require(isinstance(value, str), "private network must be an explicit CIDR string")
        network = ipaddress.ip_network(value, strict=True)
        require(network.version == 4 and (prefix is None or network.prefixlen == prefix)
                and any(network.subnet_of(ipaddress.ip_network(cidr)) for cidr in
                        ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16")),
                "unsupported private IPv4 subnet or prefix")
        return network

    @staticmethod
    def fence_rules(fence, *, maintenance=True):
        subnet = Deployment.private_network(fence["subnet"])
        return ("# piping operation " + fence["nonce"] + "\nlocal all all trust\n"
                + ("host piping piping " + str(subnet) + " scram-sha-256\n" if maintenance else "") +
                "host all all 0.0.0.0/0 reject\nhost all all ::0/0 reject\n"
                "host replication all 0.0.0.0/0 reject\nhost replication all ::0/0 reject\n").encode()

    def realize_access_fence(self, subnet=None):
        """Retain fail-closed access until an explicitly guarded startup/recovery."""
        network_subnet = self.private_network(subnet, prefix=29) if subnet is not None else None
        self.stopped()
        self.guard_writer_ids()
        self.guard_config()
        self.database_writer_guard()
        identity = self.postgres_identity()
        original = self.hba_bytes(identity)
        nonce = uuid.uuid4().hex
        name = "piping-maintenance-" + nonce
        label = "piping.release-stage=" + self.stage.name
        # Retain original bytes and the intended fence before any HBA mutation.
        # On interruption these artifacts are recovery inputs, never auto-cleaned.
        write(self.stage / "original-pg_hba.conf", original)
        network_id = run(["docker", "network", "create", "--internal",
                          *(["--subnet", str(network_subnet)] if network_subnet else []),
                          "--label", label, name]).decode().strip()
        network = json.loads(run(["docker", "network", "inspect", network_id]))[0]
        require(network["Name"] == name and network["Internal"] is True
                and network["Labels"].get("piping.release-stage") == self.stage.name
                and len(network["IPAM"]["Config"]) == 1 and not network.get("Containers"),
                "unsupported maintenance network")
        fence = {"schema": "piping.postgres-access-fence/v1", "nonce": nonce,
                 "postgres": identity, "network": network_id, "name": name,
                 "subnet": network["IPAM"]["Config"][0]["Subnet"],
                 "original_hba_sha256": sha(original), "plan_sha256": file_sha(self.stage / "plan.json")}
        rules = self.fence_rules(fence)
        write_json(self.stage / "access-fence.json", fence)
        # Install reject rules before attaching a new client path.
        self.replace_hba(identity, rules, nonce)
        require(self.hba_bytes(identity) == rules, "native access fence bytes differ")
        run(["docker", "network", "connect", "--alias", "postgres", network_id, identity["container"]])
        self.guard_access_fence()
        return fence

    def guard_access_fence(self, *, allow_missing_writers=False):
        fence = read_json(self.stage / "access-fence.json")
        require(fence["schema"] == "piping.postgres-access-fence/v1"
                and fence["plan_sha256"] == file_sha(self.stage / "plan.json")
                and self.postgres_identity() == fence["postgres"]
                and file_sha(self.stage / "original-pg_hba.conf") == fence["original_hba_sha256"],
                "retained PostgreSQL fence identity changed")
        network = json.loads(run(["docker", "network", "inspect", fence["network"]]))[0]
        require(network["Name"] == fence["name"] and network["Internal"] is True
                and network["Labels"].get("piping.release-stage") == self.stage.name
                and network["IPAM"]["Config"] == [{"Subnet": fence["subnet"],
                    "Gateway": str(ipaddress.ip_network(fence["subnet"]).network_address + 1)}]
                and set(network.get("Containers", {})) == {fence["postgres"]["container"]},
                "maintenance network identity or clients changed")
        require(self.hba_bytes(fence["postgres"]) == self.fence_rules(fence), "native access fence was removed")
        require(self.sql("SELECT NOT EXISTS (SELECT FROM pg_hba_file_rules WHERE error IS NOT NULL);") == b"t",
                "native access fence invalid")
        self.database_writer_guard()
        self.prove_tcp_rejection(fence["postgres"])
        self.prove_no_tcp_sessions(fence["postgres"])
        self.stopped()
        self.guard_writer_ids(allow_missing=allow_missing_writers)
        return fence

    def prove_tcp_rejection(self, identity):
        # This bounded product-protocol predicate synchronizes asynchronous HBA
        # activation. No fixed delay and no command output enters a public error.
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            with tempfile.TemporaryFile() as output:
                result = subprocess.run(["docker", "exec", identity["container"], "sh", "-ceu",
                'export LC_ALL=C; PGPASSWORD="$POSTGRES_PASSWORD"; export PGPASSWORD; '
                'exec psql -X -w -h 127.0.0.1 -p 5432 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT 1"'],
                    stdout=output, stderr=output, timeout=max(0.1, deadline-time.monotonic()), env=environment(), check=False)
                output.seek(0)
                if result.returncode != 0 and b"pg_hba.conf rejects connection" in output.read(MAX_BYTES):
                    return
                require(result.returncode == 0, "native TCP rejection probe failed for an unrelated reason")
        raise DeployError("native access rejection was not activated before its deadline")

    def prove_no_tcp_sessions(self, identity):
        # Kernel-established sockets include clients stalled in authentication,
        # before they necessarily appear in pg_stat_activity. Unix maintenance
        # queries remain available; an old admitted TCP session may not survive.
        count = run(["docker", "exec", identity["container"], "awk",
                     '$2 ~ /:1538$/ && $4 == "01" {n++} END {print n+0}',
                     "/proc/net/tcp", "/proc/net/tcp6"]).strip()
        require(count == b"0", "established PostgreSQL TCP sessions remain behind the access fence")

    def guard_writer_ids(self, *, allow_missing=False):
        for service, expected in self.plan["writer_ids"].items():
            self.writer_container(service, expected, allow_missing=allow_missing)

    def writer_container(self, service, expected, *, allow_missing=False):
        ids = self.writer_containers(service)
        if not ids and allow_missing:
            return None
        require(len(ids) == 1, "installed writer identity changed since staging")
        return self.validate_writer_container(service, expected, ids[0])

    def writer_containers(self, service):
        return run(["docker", "ps", "-aq", "--no-trunc", "--filter", "label=com.docker.compose.project=" + self.args.project_name,
                    "--filter", "label=com.docker.compose.service=" + service]).decode().splitlines()

    def validate_writer_container(self, service, expected, identifier):
        if identifier == expected:
            return identifier
        require(os.path.lexists(self.stage / "startup.compose.json"), "writer identity changed before controlled startup")
        fence = read_json(self.stage / "access-fence.json")
        inspected = json.loads(run(["docker", "inspect", identifier]))[0]
        labels = inspected["Config"].get("Labels", {})
        role = "backend" if service == "proxy" else service
        allowed = {self.image(role)}
        if os.path.lexists(self.stage / "original-r-manifest.json"):
            self.snapshot_manifest("original-r")
            recovery = read_json(self.stage / "recovery-images.json")
            allowed.add(recovery[role]["image_id"])
        elif os.path.lexists(self.stage / "backup-manifest.json") or os.path.lexists(self.stage / "cutover-r-manifest.json"):
            kind = getattr(self, "recovery_kind", "backup" if os.path.lexists(self.stage / "backup-manifest.json") else "cutover-r")
            self.snapshot_manifest(kind)
            allowed.add(read_json(self.stage / "recovery-images.json")[role]["image_id"])
        require(labels.get("piping.preparation-start") == fence["nonce"]
                    and labels.get("com.docker.compose.project") == self.args.project_name
                    and labels.get("com.docker.compose.service") == service and inspected["Image"] in allowed,
                    "unrecognized replacement writer container")
        return identifier

    def retain_access_fence(self, *, allow_missing=False):
        """Failure path: close access first, then stop only positively owned clients."""
        self.guard_latest_stage()
        if os.path.lexists(self.stage / "original-r-manifest.json"):
            self.snapshot_manifest("original-r")
        elif os.path.lexists(self.stage / "backup-manifest.json") or os.path.lexists(self.stage / "cutover-r-manifest.json"):
            kind = getattr(self, "recovery_kind", "backup" if os.path.lexists(self.stage / "backup-manifest.json") else "cutover-r")
            self.snapshot_manifest(kind)
        else:
            authenticate(self.stage / "applied-restore-receipt.json", self.stage / "applied-restore-receipt.sig",
                         self.stage / "verification-key.pem")
            receipt = read_json(self.stage / "applied-restore-receipt.json")
            require(receipt["request_sha256"] == file_sha(self.stage / "backup-request.json")
                    and receipt["request"] == read_json(self.stage / "backup-request.json"),
                    "refence authority differs from the consumed restore proof")
            for name in ("access-fence.json", "original-pg_hba.conf", "startup.compose.json"):
                require(file_sha(self.stage / name) == receipt["request"]["files"][name]["sha256"],
                        "authenticated retained fence member changed")
        fence = read_json(self.stage / "access-fence.json")
        require(self.postgres_identity(fence["postgres"]["container"]) == fence["postgres"],
                "cannot refence a substituted PostgreSQL provider")
        self.replace_hba(fence["postgres"], self.fence_rules(fence, maintenance=False), uuid.uuid4().hex)
        unsafe = False
        try:
            network = json.loads(run(["docker", "network", "inspect", fence["network"]]))[0]
            for identifier in set(network.get("Containers", {})) - {fence["postgres"]["container"]}:
                inspected = json.loads(run(["docker", "inspect", identifier]))[0]
                require(inspected["Config"].get("Labels", {}).get("piping.fence") == fence["nonce"]
                    and inspected["Image"] == self.image("backend")
                    and inspected["Name"].startswith("/piping-maintenance-client-"),
                    "unknown client attached to retained maintenance network")
                run(["docker", "rm", "-f", identifier])
        except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError):
            # Do not let an unknown network peer bypass containment of known
            # writers or keep already established target sessions alive.
            unsafe = True
        for service, expected in self.plan["writer_ids"].items():
            try:
                identifiers = self.writer_containers(service)
                if not identifiers and not allow_missing or len(identifiers) > 1:
                    unsafe = True
            except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError):
                unsafe = True
                continue
            for identifier in identifiers:
                try:
                    self.validate_writer_container(service, expected, identifier)
                    run(["docker", "stop", "--time", "30", identifier], timeout=60)
                except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError):
                    # Unknown containers are not ours to stop; contain every
                    # independently identified surviving writer regardless.
                    unsafe = True
        # Explicit maintenance authority covers this pinned target database's
        # connections, including clients reconnecting during controlled startup.
        activated = False
        try:
            # Prove reject rules are active before taking the termination
            # snapshot, including when an unknown writer keeps reconnecting.
            self.prove_tcp_rejection(fence["postgres"])
            activated = True
        except (ValueError, OSError, subprocess.SubprocessError):
            unsafe = True
        try:
            require(activated, "deny-all activation is uncertain")
            self.pinned_postgres_sql(fence["postgres"]["container"],
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname=current_database() "
                "AND backend_type='client backend' AND pid<>pg_backend_pid();")
            require(self.pinned_postgres_sql(fence["postgres"]["container"],
                "SELECT NOT EXISTS (SELECT FROM pg_stat_activity WHERE datname=current_database() "
                "AND backend_type='client backend' AND pid<>pg_backend_pid());") == b"t",
                "target database sessions remain after containment")
            self.prove_no_tcp_sessions(fence["postgres"])
        except (ValueError, OSError, subprocess.SubprocessError):
            # Pre-authentication sockets may not be in pg_stat_activity yet.
            # This provider was authenticated above; never stop a substitute.
            run(["docker", "stop", "--time", "30", fence["postgres"]["container"]], timeout=60)
            require(not json.loads(run(["docker", "inspect", fence["postgres"]["container"]]))[0]["State"]["Running"],
                    "pinned PostgreSQL provider did not stop during containment")
            write_json(self.stage / ("provider-containment-" + uuid.uuid4().hex + ".json"),
                       {"container": fence["postgres"]["container"], "nonce": fence["nonce"], "stopped": True})
            raise DeployError("pinned PostgreSQL provider stopped: complete connection containment could not be proven") from None
        for name, digest in self.plan["baseline"].items():
            allowed = {digest}
            if name == self.plan["image_file"]:
                allowed.add(file_sha(self.stage / "images.next"))
            elif name in self.plan.get("candidate_config", {}):
                allowed.add(file_sha(self.stage / self.plan["candidate_config"][name]))
            if file_sha(self.root / name) not in allowed:
                unsafe = True
        require(not unsafe, "partial containment: known writers stopped and target sessions terminated; deny-all fence retained")
        self.replace_hba(fence["postgres"], self.fence_rules(fence), uuid.uuid4().hex)
        self.guard_access_fence(allow_missing_writers=allow_missing)

    def controlled_start(self, *, original=False):
        fence = self.guard_access_fence(allow_missing_writers=original)
        writers = self.controlled_writers()
        expected_overlay = {"services": {role: {"labels": {"piping.preparation-start": fence["nonce"]}} for role in writers}}
        require(read_json(self.stage / "startup.compose.json") == expected_overlay, "startup ownership overlay changed")
        base = json.loads(run(self.compose() + ["config", "--format", "json"]))
        command = self.compose(startup=self.stage / "startup.compose.json")
        model = json.loads(run(command + ["config", "--format", "json"]))
        for role in writers:
            base["services"][role].setdefault("labels", {})["piping.preparation-start"] = fence["nonce"]
        require(base == model, "startup overlay changes more than ownership labels")
        activation = self.guarded_activation()
        if activation is not None:
            self.guard_job_network_start()
            self.guarded_runner_stop()
        self.replace_hba(fence["postgres"], read(self.stage / "original-pg_hba.conf"), uuid.uuid4().hex)
        require(self.hba_bytes(fence["postgres"]) == read(self.stage / "original-pg_hba.conf"),
                "original native access rules were not restored")
        try:
            startup_writers = [role for role in writers if activation is None or role != "ansible-runner"]
            run(command + ["up", "-d", "--wait", "--wait-timeout", "180", "--no-deps", "--pull", "never",
                           "--no-build", *startup_writers, "frontend"], timeout=210)
            if activation is not None:
                self.guarded_runner_start(recreate=True, startup=self.stage / "startup.compose.json")
            require(set(writers + ["frontend"]) <= self.running(), "partial controlled startup")
            self.guard_writer_ids()
            self.guard_config(installed=not original)
        except BaseException:
            self.retain_access_fence()
            raise

    def controlled_writers(self):
        identities = self.plan["writer_ids"]
        running = self.plan["writer_running"]
        require(isinstance(identities, dict) and {"backend", "ansible-runner"} <= set(identities) <= set(WRITERS)
                and all(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) for value in identities.values())
                and isinstance(running, dict) and set(running) == set(identities)
                and all(type(value) is bool for value in running.values()), "invalid staged writer identities/states")
        writers = read_json(self.stage / "writers.json")
        require(isinstance(writers, list) and writers == [role for role in WRITERS if running.get(role) is True]
                and {"backend", "ansible-runner"} <= set(writers), "writer startup set differs from the staged state")
        fence = read_json(self.stage / "access-fence.json")
        require(isinstance(fence["nonce"], str) and re.fullmatch(r"[0-9a-f]{32}", fence["nonce"])
                and read_json(self.stage / "startup.compose.json") == {"services": {
                    role: {"labels": {"piping.preparation-start": fence["nonce"]}} for role in writers}},
                "startup overlay exceeds the declared writer set")
        return writers

    def resume_contained_provider(self):
        fence = read_json(self.stage / "access-fence.json")
        inspected = json.loads(run(["docker", "inspect", fence["postgres"]["container"]]))[0]
        if inspected["State"]["Running"]:
            return
        markers = [read_json(path) for path in self.stage.glob("provider-containment-*.json")]
        require(any(marker == {"container": fence["postgres"]["container"], "nonce": fence["nonce"], "stopped": True}
                    for marker in markers), "stopped PostgreSQL provider has no operation-owned containment record")
        require(inspected["Id"] == fence["postgres"]["container"] and inspected["Image"] == fence["postgres"]["image"]
                and inspected["Mounts"] == fence["postgres"]["mounts"]
                and inspected["Config"].get("Labels", {}).get("com.docker.compose.project") == self.args.project_name
                and inspected["Config"].get("Labels", {}).get("com.docker.compose.service") == "postgres",
                "contained PostgreSQL container/storage was substituted")
        archive = run(["docker", "cp", fence["postgres"]["container"] + ":" + fence["postgres"]["hba_file"], "-"])
        with tarfile.open(fileobj=io.BytesIO(archive)) as source:
            members = source.getmembers()
            require(len(members) == 1 and members[0].isfile() and members[0].size < 1024 * 1024
                    and source.extractfile(members[0]).read() == self.fence_rules(fence, maintenance=False),
                    "contained provider must retain deny-all HBA before explicit restart")
        run(["docker", "start", fence["postgres"]["container"]])
        # No automatic startup of writers. If PostgreSQL is not ready yet the
        # caller refuses without advancing the recovery journal; deny-all stays.

    def restore_original_r(self):
        self.restore_snapshot("original-r")

    def restore_backup(self):
        self.restore_snapshot("backup")

    def restore_cutover_r(self):
        self.restore_snapshot("cutover-r")

    def snapshot_manifest(self, kind, *, authenticated=True):
        return self.snapshot_manifest_data(kind, helper_identity(), authenticated=authenticated)

    def snapshot_manifest_data(self, kind, authority_helper, *, authenticated):
        require(kind in ("original-r", "backup", "cutover-r"), "invalid recovery source")
        value = read_json(self.stage / (kind + "-manifest.json"))
        original = kind == "original-r"
        keys = {"schema", "plan_sha256", "helper_sha256", "database_bytes", "files"}
        require(set(value) == (keys if original else keys | {"source"})
                and value["schema"] == ("piping.fhrp-original-r/v1" if original else "piping.compose-recovery/v1")
                and (original or value["source"] == kind)
                and value["plan_sha256"] == file_sha(self.stage / "plan.json")
                and value["helper_sha256"] == authority_helper == self.plan["helper_sha256"]
                and type(value["database_bytes"]) is int and value["database_bytes"] > 0
                and set(value["files"]) == self.snapshot_members(kind), "recovery manifest mismatch")
        for name, digest in value["files"].items():
            require(Path(name).name == name and file_sha(self.stage / name) == digest,
                    "recovery member identity mismatch")
        require(all(value["files"][name + ".original"] == digest for name, digest in self.plan["baseline"].items()),
                "recovery configuration differs from staging")
        self.controlled_writers()
        if authenticated:
            if original or kind == "cutover-r":
                stem = "original-r" if original else "cutover-r"
                receipt_path, signature = self.stage / (stem + "-receipt.json"), self.stage / (stem + "-receipt.sig")
                request_name = stem + "-request.json"
            else:
                receipt_path, signature = getattr(self.args, "receipt", None), getattr(self.args, "receipt_signature", None)
                require((receipt_path is None) == (signature is None), "recovery receipt and signature must be paired")
                if receipt_path is None:
                    receipt_path, signature = self.stage / "applied-restore-receipt.json", self.stage / "applied-restore-receipt.sig"
                request_name = "backup-request.json"
            receipt = self.restore_receipt_data(receipt_path, signature, request_name, authority_helper)
            require(receipt["request"]["files"][kind + "-manifest.json"]["sha256"] == file_sha(self.stage / (kind + "-manifest.json")),
                    "recovery manifest is not authenticated by its native restore proof")
            if original:
                require(receipt["request"]["preparation"] == {
                    "schema": "piping.fhrp-preparation/v1", "phase": "original-r",
                    "original_manifest_sha256": file_sha(self.stage / "original-r-manifest.json")},
                    "original R receipt is not a preparation authorization")
            elif kind == "cutover-r":
                require(receipt["request"].get("cutover") == {"schema": "piping.automation-cutover/v1", "phase": "original-r"}
                        and receipt.get("cutover_proof") == {"schema": "piping.automation-cutover/v1", "result": "original-r-conserved"},
                        "original automation R has no native conservation proof")
        return value

    def restore_snapshot(self, kind):
        phase = self.phase()
        require("recovering-" + kind in PHASE_EDGES.get(phase, ()), "recovery is unavailable or already attempted")
        self.load(phase)
        self.recovery_kind = kind
        self.snapshot_manifest(kind)
        if kind == "backup" and not os.path.lexists(self.stage / "applied-restore-receipt.json"):
            write(self.stage / "applied-restore-receipt.json", read(self.args.receipt))
            write(self.stage / "applied-restore-receipt.sig", read(self.args.receipt_signature))
        # Configuration may only be exactly staged-original or exactly the
        # candidate image assignment; unknown substitutions are never repaired.
        for name, digest in self.plan["baseline"].items():
            allowed = {digest}
            if name == self.plan["image_file"]:
                allowed.add(file_sha(self.stage / "images.next"))
            elif name in self.plan.get("candidate_config", {}):
                allowed.add(file_sha(self.stage / self.plan["candidate_config"][name]))
            require(file_sha(self.root / name) in allowed, "configuration substitution blocks matched recovery")
        self.guard_active_overlay()
        # A partial install can contain known original/candidate file mixtures.
        # Resolve the authenticated originals to detect external env/include
        # drift before changing images or data, rather than accepting that mix.
        require(sha(canonical(json.loads(run(self.compose(original=True) + ["config", "--format", "json"]))))
                == self.plan["original_model_sha256"], "resolved original configuration changed before recovery")
        self.resume_contained_provider()
        self.retain_access_fence(allow_missing=True)
        fence = self.guard_access_fence(allow_missing_writers=True)
        self.guard_recovery_capacity(max(self.snapshot_manifest(kind)["database_bytes"],
                                        int(self.sql("SELECT pg_database_size(current_database());"))))
        images = read_json(self.stage / "recovery-images.json")
        require(set(images) == set(ROLES), "incomplete original image set")
        for role, image in images.items():
            loaded = load_image(self.stage, image["archive"], self.plan["platform"])
            if "@" not in image["reference"] and not image["reference"].startswith("sha256:"):
                run(["docker", "image", "tag", loaded, image["reference"]])
            inspected = json.loads(run(["docker", "image", "inspect", image["reference"]]))[0]
            require(inspected["Id"] == image["image_id"], "original image reference was not restored exactly")
        archive = "offline-pre-upgrade.dump" if kind == "backup" else kind + ".dump"
        fingerprint = "fingerprint.json" if kind == "backup" else kind + "-fingerprint.json"
        original = read_json(self.stage / fingerprint)
        operation = uuid.uuid4().hex
        restored_name = "piping_recovery_" + operation
        quarantine = "piping_quarantine_" + operation
        require(self.sql("SELECT NOT EXISTS (SELECT FROM pg_database WHERE datname IN ('" + restored_name
                         + "','" + quarantine + "'));") == b"t", "recovery/quarantine identity already exists")
        self.advance("recovering-" + kind)
        self.sql("CREATE DATABASE " + restored_name + ";")
        database = {"compose": self.compose(), "name": restored_name}
        self.raw_restore(database, self.stage / archive, original)
        self.snapshot_manifest(kind)
        self.guard_access_fence(allow_missing_writers=True)
        # The failed target remains retained under an operation-bound identity.
        run(self.postgres_command("psql", "-X", "-qAt", "-v", "ON_ERROR_STOP=1", "-U", "piping", "-d", "postgres"),
            data=("BEGIN; ALTER DATABASE piping ALLOW_CONNECTIONS false; ALTER DATABASE piping RENAME TO " + quarantine
                  + "; ALTER DATABASE " + restored_name + " RENAME TO piping; COMMIT;").encode())
        for name in self.plan["baseline"]:
            temporary = self.root / (name + ".recovery-" + fence["nonce"])
            write(temporary, read(self.stage / (name + ".original")))
            target = self.root / name
            os.replace(temporary, target)
            sync_directory(target.parent)
        sync_directory(self.root)
        self.guard_config()
        require(self.fingerprint() == {**original, "catalog": self.plan["restored_target_catalog"]},
                "recovered snapshot changed during database switch")
        self.guard_access_fence(allow_missing_writers=True)
        write_json(self.stage / (kind + "-recovery.json"), {"source": kind, "manifest_sha256": file_sha(self.stage / (kind + "-manifest.json")),
                    "quarantine": quarantine, "database": "piping", "writers_stopped": True})
        self.advance(kind + "-restored")

    def start_original_r(self):
        self.start_snapshot("original-r")

    def start_backup(self):
        self.start_snapshot("backup")

    def start_cutover_r(self):
        self.start_snapshot("cutover-r")

    def start_snapshot(self, kind):
        self.load(kind + "-restored")
        self.recovery_kind = kind
        self.snapshot_manifest(kind)
        self.guard_config()
        self.guard_access_fence(allow_missing_writers=True)
        original = read_json(self.stage / ("fingerprint.json" if kind == "backup" else kind + "-fingerprint.json"))
        require(self.fingerprint() == {**original, "catalog": self.plan["restored_target_catalog"]},
                "restored snapshot changed before explicit startup")
        self.advance("starting-" + kind)
        try:
            self.controlled_start(original=True)
            self.runtime_health({role: image["image_id"] for role, image in
                                 read_json(self.stage / "recovery-images.json").items()})
            self.guard_database()
            self.advance(kind + "-started")
        except BaseException:
            self.retain_access_fence()
            raise

    def recovery_image_inputs(self):
        model = json.loads(run(self.compose() + ["config", "--format", "json"]))
        images = {}
        for role in ROLES:
            image = self.service_image(role)
            reference = model["services"][role]["image"]
            require(isinstance(reference, str) and reference and not reference.startswith("-"),
                    "invalid installed image reference")
            require("@" not in reference, "repository digest references lack supported offline recovery; preparation refused")
            images[role] = {"image_id": image, "reference": reference,
                            "archive": save_image(image, self.stage, "recovery-" + role, None, self.plan["platform"])}
        write_json(self.stage / "recovery-images.json", images)

    def guard_recovery_capacity(self, size):
        identity = self.postgres_identity()
        free = run(["docker", "exec", identity["container"], "sh", "-ceu", 'exec df -Pk -- "$1"',
                    "postgres-space", identity["data_directory"]]).decode().splitlines()
        require(len(free) == 2 and len(free[1].split()) >= 6, "cannot establish PostgreSQL storage capacity")
        available = int(free[1].split()[-3]) * 1024
        require(available >= max(size * 3, 1024 * 1024 * 1024),
                "insufficient PostgreSQL storage for retained quarantine and recovered database")


    def prepare_fhrp_auth_keys(self):
        if self.phase() == "staged":
            require(self.args.receipt is None and self.args.receipt_signature is None,
                    "initial R capture must not consume an unrelated receipt")
            self.backup(prepare_original=True)
            return
        self.load("original-r-awaiting-proof")
        require(self.args.receipt is not None and self.args.receipt_signature is not None,
                "preparation awaits the independent original R restore receipt")
        self.verify_seal("original-r-seal.json")
        receipt = self.validate_restore_receipt(self.args.receipt, self.args.receipt_signature, "original-r-request.json")
        require(receipt["request"]["preparation"]["phase"] == "original-r"
                and set(receipt["preparation_proof"]) == {"schema", "semantics"}
                and receipt["preparation_proof"]["schema"] == "piping.fhrp-preparation/v1",
                "missing independent original R conversion proof")
        self.guard_runtime_inputs()
        fence = self.guard_access_fence()
        require(str(ipaddress.ip_network(self.args.maintenance_subnet, strict=True)) == fence["subnet"],
                "resume maintenance subnet differs from the retained fence")
        original = read_json(self.stage / "original-r-fingerprint.json")
        require(self.fingerprint() == original, "original source changed while awaiting proof")
        source = read_json(self.stage / "original-r-source.json")
        self.validate_fhrp_source(source)
        write(self.stage / "original-r-receipt.json", read(self.args.receipt))
        write(self.stage / "original-r-receipt.sig", read(self.args.receipt_signature))
        self.snapshot_manifest("original-r")
        self.advance("original-r-verified")
        self.advance("converting-fhrp")
        self.maintenance_run(self.fenced_target(), convert=True)
        source = read_json(self.stage / "original-r-source.json")
        require(self.verify_fhrp_conversion(source) == receipt["preparation_proof"]["semantics"],
                "target conversion differs from independent R replay semantics")
        prepared = self.fingerprint()
        self.maintenance_run(self.fenced_target(), convert=True)
        require(self.fingerprint() == prepared, "repeating prepared conversion changed state")
        self.guard_access_fence()
        self.snapshot_manifest("original-r")
        self.advance("prepared-c-verified")
        request = receipt["request"]
        self.complete_backup(request["database_bytes"], request["postgres_archive"],
                             request["installed_backend_archive"], prepare_original=True)

    def prepare_original_r(self, size, postgres_archive, installed_archive):
        require("FHRP0004" in self.transforms(), "no exact pending FHRP transform")
        self.advance("original-r-dumping")
        original = self.raw_archive("original-r.dump", "original-r-fingerprint.json")
        source = self.fhrp_source_state()
        require(source["pending"], "no original nonempty FHRP values; use prepared-C backup")
        write_json(self.stage / "original-r-source.json", source)
        size = int(self.sql("SELECT pg_database_size(current_database());"))
        files = self.snapshot_members("original-r")
        with self.isolated_database("original-r-proof") as database:
            self.raw_restore(database, self.stage / "original-r.dump", original)
            self.write_snapshot_manifest("original-r", size)
            self.snapshot_manifest("original-r", authenticated=False)
        self.guard_access_fence()
        require(self.fingerprint() == original, "original source changed during local restore proof")
        self.write_backup_request(size, postgres_archive, installed_archive,
            ["original-r-manifest.json", *sorted(files)], preparation_phase="original-r")
        write(self.stage / "original-r-request.json", read(self.stage / "backup-request.json"))
        self.seal("original-r-seal.json", ["backup-request.json", "original-r-request.json", "writers.json",
                  "original-r-manifest.json", *sorted(files), postgres_archive["archive"], installed_archive["archive"]])
        self.advance("original-r-awaiting-proof")

    def snapshot_members(self, kind):
        require(kind in ("original-r", "backup", "cutover-r"), "invalid recovery source")
        archive, fingerprint = (("offline-pre-upgrade.dump", "fingerprint.json") if kind == "backup"
                                else (kind + ".dump", kind + "-fingerprint.json"))
        members = {archive, fingerprint, "runtime-inputs.json", "access-fence.json", "original-pg_hba.conf",
                   "proof-backend.env", "startup.compose.json", "writers.json", "recovery-images.json",
                   "installed-target-schema.dump", *(name + ".original" for name in self.plan["baseline"]),
                   *self.plan.get("candidate_config", {}).values()}
        members.add("original-r-source.json" if kind == "original-r" else "images.next")
        if self.plan.get("active_overlay"):
            members.add("active-overlay.original")
        if kind == "cutover-r":
            members.add("cutover-preservation.json")
        images = read_json(self.stage / "recovery-images.json")
        require(set(images) == set(ROLES), "incomplete recovery image set")
        for role, image in images.items():
            require(set(image) == {"image_id", "reference", "archive"}
                    and image["archive"]["archive"] == "recovery-" + role + ".tar",
                    "invalid recovery image identity")
            members.add(image["archive"]["archive"])
        return members

    def write_snapshot_manifest(self, kind, size):
        value = {"schema": "piping.fhrp-original-r/v1" if kind == "original-r" else "piping.compose-recovery/v1",
                 "plan_sha256": file_sha(self.stage / "plan.json"), "helper_sha256": helper_identity(),
                 "database_bytes": size, "files": {name: file_sha(self.stage / name) for name in self.snapshot_members(kind)}}
        if kind != "original-r":
            value["source"] = kind
        write_json(self.stage / (kind + "-manifest.json"), value)


    def cutover_binding_digest(self):
        return self.sql("SELECT count(*)::text || ':' || "
            "encode(sha256(convert_to(coalesce(string_agg(h,'' ORDER BY h),''),'UTF8')),'hex') FROM "
            "(SELECT encode(sha256(convert_to(to_jsonb(source)::text,'UTF8')),'hex') AS h "
            "FROM public.secret_usage_binding AS source WHERE consumer_type NOT IN "
            "('playbook_version:runtime_env','playbook_project')) hashed;").decode()

    def capture_cutover_r(self):
        self.backup(capture_cutover=True)

    def complete_cutover_r(self, size, postgres_archive, installed_archive):
        self.advance("cutover-r-dumping")
        self.raw_archive("cutover-r.dump", "cutover-r-fingerprint.json")
        write_json(self.stage / "cutover-preservation.json", {
            "schema": "piping.automation-retirement-conservation/v1",
            "surviving_secret_usage_binding": self.cutover_binding_digest()})
        self.write_snapshot_manifest("cutover-r", size)
        members = sorted(self.snapshot_members("cutover-r") | {"cutover-r-manifest.json"})
        self.write_backup_request(size, postgres_archive, installed_archive, members, cutover_phase="original-r")
        write(self.stage / "cutover-r-request.json", read(self.stage / "backup-request.json"))
        self.seal("cutover-r-seal.json", [*members, "cutover-r-request.json", postgres_archive["archive"],
                  *([installed_archive["archive"]] if installed_archive else [])])
        self.advance("cutover-r-awaiting-proof")

    def prove_cutover_r_request(self):
        self.snapshot_manifest("cutover-r", authenticated=False)
        original = read_json(self.stage / "cutover-r-fingerprint.json")
        with self.isolated_database("automation-original-r") as database:
            self.raw_restore(database, self.stage / "cutover-r.dump", original)
            with self.database_target(database):
                require(read_json(self.stage / "cutover-preservation.json") == {
                    "schema": "piping.automation-retirement-conservation/v1",
                    "surviving_secret_usage_binding": self.cutover_binding_digest()},
                    "original R surviving binding projection differs from native restore")
        return {"schema": "piping.automation-cutover/v1", "result": "original-r-conserved"}

    def accept_cutover_r(self):
        self.load("cutover-r-awaiting-proof")
        self.verify_seal("cutover-r-seal.json")
        receipt = self.validate_restore_receipt(self.args.receipt, self.args.receipt_signature, "cutover-r-request.json")
        require(receipt["request"].get("cutover") == {"schema": "piping.automation-cutover/v1", "phase": "original-r"}
                and receipt.get("cutover_proof") == {"schema": "piping.automation-cutover/v1", "result": "original-r-conserved"},
                "wrong proof for automation original R")
        self.guard_config()
        self.guard_runtime_inputs()
        self.guard_access_fence()
        require(self.fingerprint() == read_json(self.stage / "cutover-r-fingerprint.json"),
                "original R changed before proof acceptance")
        write(self.stage / "cutover-r-receipt.json", read(self.args.receipt))
        write(self.stage / "cutover-r-receipt.sig", read(self.args.receipt_signature))
        self.snapshot_manifest("cutover-r")
        self.advance("cutover-r-verified")

    def guard_cutover_conservation(self, prepared):
        """The operator deletes retired rows; the updater only verifies that boundary."""
        require(self.workspace_cutover(), "automation retirement requires the exact workspace transition")
        original = read_json(self.stage / "cutover-r-fingerprint.json")
        require(all(prepared[key] == original[key] for key in ("catalog", "columns", "history"))
                and set(prepared["rows"]) == set(original["rows"]), "automation cleanup changed schema or ledger")
        allowed = {*WORKSPACE_0016_RETIRED_TABLES, "public.secret_usage_binding"}
        require(all(value == prepared["rows"][table] for table, value in original["rows"].items() if table not in allowed),
                "automation cleanup changed retained application data")
        require(prepared["rows"]["public.secret_usage_binding"] == read_json(self.stage / "cutover-preservation.json")["surviving_secret_usage_binding"],
                "automation cleanup changed retained secret bindings")
        self.guard_workspace_prepared(prepared)

    def backup(self, *, prepare_original=False, capture_cutover=False):
        if not prepare_original and not capture_cutover and self.phase() == "cutover-r-verified":
            self.load("cutover-r-verified")
            self.verify_seal("cutover-r-seal.json")
            self.snapshot_manifest("cutover-r")
            self.guard_config()
            self.guard_runtime_inputs()
            self.guard_access_fence()
            self.inspect_images()
            self.guard_cutover_conservation(self.fingerprint())
            request = read_json(self.stage / "cutover-r-request.json")
            self.complete_backup(int(self.sql("SELECT pg_database_size(current_database());")),
                                 request["postgres_archive"], request["installed_backend_archive"], cutover=True)
            return
        self.load("staged")
        self.guard_config()
        self.guard_writer_ids()
        self.inspect_images()
        self.guard_database()
        self.guard_target_catalog()
        if capture_cutover:
            require(not prepare_original and self.workspace_cutover() and self.plan["disabled_schedule"] is None,
                    "original automation R requires the exact workspace transition without a per-schedule guard")
        if prepare_original:
            require("FHRP0004" in self.transforms(), "no exact pending original-R FHRP transform")
            require(self.sql("SELECT EXISTS (SELECT FROM ipam_fhrpgroup WHERE auth_key<>'');") == b"t",
                    "no original nonempty FHRP values; use prepared-C backup")
        else:
            # Reject a wrong public action before stopping writers. The sealed
            # expectation repeats these checks under the realized fence, so a
            # concurrent pre-quiescence change cannot authorize migration.
            if "FHRP0004" in self.transforms():
                self.sql(FHRP_CUTOVER_VALIDATE_SQL)
            if self.service_cutover():
                self.sql(SERVICE_CUTOVER_VALIDATE_SQL)
        require(self.service_image("postgres") == self.plan["postgres_image"], "installed PostgreSQL image changed")
        write_json(self.stage / "runtime-inputs.json", self.runtime_inputs())
        # Read-only tool/capacity preflight precedes writer stop. Image exports are
        # carried to the leader too; no registry availability is assumed there.
        run(["openssl", "version"])
        size = int(self.sql("SELECT pg_database_size(current_database());"))
        self.guard_recovery_capacity(size)
        require(shutil.disk_usage(self.stage).free >= max(size * 3, 1024 * 1024 * 1024),
                "insufficient disk capacity for backup and native image archives")
        postgres_archive = save_image(self.plan["postgres_image"], self.stage, "installed-postgres", None,
                                      self.plan["platform"])
        installed_archive = None
        if len(self.plan["installed_history"]) < len(self.plan["migrations"]):
            installed_archive = save_image(self.plan["installed_backend_image"], self.stage, "installed-backend", None,
                                            self.plan["platform"])
        if prepare_original or capture_cutover:
            require(shutil.disk_usage(self.stage).free >= max(size * 5, 1024 * 1024 * 1024),
                    "insufficient capacity for R/C, restores and quarantined state")
        self.recovery_image_inputs()
        require(shutil.disk_usage(self.stage).free >= max(size * 2, 1024 * 1024 * 1024),
                "insufficient disk capacity after image export")
        writers = [service for service in WRITERS if service in self.running()]
        require(
            set(writers) >= {"backend", "ansible-runner"},
            "required writers are not running",
        )
        require(writers == [service for service in WRITERS if self.plan["writer_running"].get(service) is True],
                "writer running state changed since staging")
        self.advance("backup-starting")
        write_json(self.stage / "writers.json", writers)
        self.advance("writers-stopping")
        if self.guarded_activation() is not None:
            # Stop the immutable-ID unit before Compose stops its container, so a
            # failed attach cannot be restarted by the unit outside the gate.
            self.guarded_runner_stop()
        run(self.compose() + ["stop", "--timeout", "60", *writers])
        self.advance("writers-stopped")
        self.stopped()
        self.guard_runtime_inputs()
        self.realize_access_fence(getattr(self.args, "maintenance_subnet", None))
        nonce = read_json(self.stage / "access-fence.json")["nonce"]
        write_json(self.stage / "startup.compose.json", {"services": {
            role: {"labels": {"piping.preparation-start": nonce}} for role in writers}})
        self.advance("access-fenced")
        if prepare_original:
            self.prepare_original_r(size, postgres_archive, installed_archive)
            return
        if capture_cutover:
            self.complete_cutover_r(size, postgres_archive, installed_archive)
            return
        self.complete_backup(size, postgres_archive, installed_archive)

    def complete_backup(self, size, postgres_archive, installed_archive, *, prepare_original=False, cutover=False):
        self.guard_database()
        self.guard_config()
        self.guard_target_catalog()
        fingerprint = self.fingerprint()
        self.guard_workspace_prepared(fingerprint)
        transformation_files = ["runtime-inputs.json", "access-fence.json", "original-pg_hba.conf", "startup.compose.json", "writers.json"]
        if prepare_original:
            original = self.snapshot_manifest("original-r")
            transformation_files.extend(["original-r-manifest.json", "original-r-request.json",
                "original-r-receipt.json", "original-r-receipt.sig", *sorted(original["files"])])
            transformation_files = sorted(set(transformation_files))
        if cutover:
            self.snapshot_manifest("cutover-r")
            self.guard_cutover_conservation(fingerprint)
            transformation_files.extend(["cutover-r-manifest.json", "cutover-r-request.json", "cutover-r-receipt.json",
                                         "cutover-r-receipt.sig", *self.snapshot_members("cutover-r")])
        if self.transforms():
            write_json(self.stage / "migration-expectation.json", self.migration_expectation(fingerprint))
            transformation_files.append("migration-expectation.json")
        self.advance("dumping")
        require(self.raw_archive("offline-pre-upgrade.dump", "fingerprint.json") == fingerprint,
                 "database changed after native expectation")
        self.write_snapshot_manifest("backup", size)
        transformation_files = sorted(set(transformation_files) | self.snapshot_members("backup") | {"backup-manifest.json"})
        self.guard_database()
        self.stopped()
        self.guard_config()
        self.write_backup_request(size, postgres_archive, installed_archive,
            ["offline-pre-upgrade.dump", "fingerprint.json", "proof-backend.env", "installed-target-schema.dump",
             *transformation_files], preparation_phase="prepared-c" if prepare_original else None,
             cutover_phase="prepared-c" if cutover else None)
        self.seal("backup-seal.json", ["offline-pre-upgrade.dump", "fingerprint.json", "writers.json", "backup-request.json",
                  *transformation_files, postgres_archive["archive"], *([installed_archive["archive"]] if installed_archive else [])])
        self.advance("backup-verified")

    def write_backup_request(self, size, postgres_archive, installed_archive, files, *, preparation_phase=None, cutover_phase=None):
        require(preparation_phase in (None, "original-r", "prepared-c"), "unsupported preparation request phase")
        require(cutover_phase in (None, "original-r", "prepared-c") and not (cutover_phase and preparation_phase),
                "automation retirement and FHRP authorities cannot be combined")
        request = {"schema": "piping.compose-backup/v1", "nonce": uuid.uuid4().hex,
                   "target": socket.gethostname(), "root": str(self.root), "stage": str(self.stage),
                   "project_name": self.args.project_name, "helper_sha256": helper_identity(),
                   "commit": self.plan["commit"], "pipeline_id": self.plan["pipeline_id"],
                   "package_sha256": self.plan["package_sha256"], "plan_sha256": file_sha(self.stage / "plan.json"),
                   "preservation_sha256": self.plan["preservation_sha256"],
                   "postgres_archive": postgres_archive, "installed_backend_archive": installed_archive,
                    "database_bytes": size,
                     "preparation": ({"schema": "piping.fhrp-preparation/v1",
                                      "phase": preparation_phase,
                                      "original_manifest_sha256": file_sha(self.stage / "original-r-manifest.json")}
                                     if preparation_phase else None),
                    "cutover": {"schema": "piping.automation-cutover/v1", "phase": cutover_phase} if cutover_phase else None,
                    "files": {name: {"sha256": file_sha(self.stage / name), "size": (self.stage / name).stat().st_size}
                              for name in (*files,
                                           *(["preservation.json"] if self.plan["preservation_sha256"] else []),
                                           *(["check-capture.json"] if self.plan["check_capture_sha256"] else []))}}
        if preparation_phase == "prepared-c" or cutover_phase == "prepared-c":
            write_json(self.stage / "prepared-c-request.json", request)
            os.replace(self.stage / "prepared-c-request.json", self.stage / "backup-request.json")
            sync_directory(self.stage)
        else:
            write_json(self.stage / "backup-request.json", request)

    def validate_restore_receipt(self, receipt_path, signature, request_name):
        return self.restore_receipt_data(receipt_path, signature, request_name, helper_identity())

    def restore_receipt_data(self, receipt_path, signature, request_name, authority_helper):
        authenticate(receipt_path, signature, getattr(self, "verifier_key", self.stage / "verification-key.pem"))
        receipt = read_json(receipt_path)
        request = read_json(self.stage / request_name)
        result = ("native-restore-conservation-passed" if request.get("cutover") == {
            "schema": "piping.automation-cutover/v1", "phase": "original-r"} else "native-restore-migration-conservation-passed")
        require(receipt["schema"] == "piping.compose-restore-receipt/v1"
                and receipt["result"] == result
                and receipt["request_sha256"] == file_sha(self.stage / request_name)
                and receipt["request"] == request
                and request["helper_sha256"] == authority_helper == self.plan["helper_sha256"]
                and request["package_sha256"] == self.plan["package_sha256"]
                and request["preservation_sha256"] == self.plan["preservation_sha256"]
                and request["plan_sha256"] == file_sha(self.stage / "plan.json")
                and request["target"] == self.plan["target"]
                and request["root"] == self.plan["root"] and request["project_name"] == self.plan["project_name"]
                and request["stage"] == read_json(self.stage / "backup-request.json")["stage"]
                and (getattr(self, "verifier", False) or request["stage"] == str(self.stage))
                and request["commit"] == self.plan["commit"] and request["pipeline_id"] == self.plan["pipeline_id"],
                "restore receipt binding mismatch")
        for name, description in request["files"].items():
            require(Path(name).name == name and file_sha(self.stage / name) == description["sha256"]
                    and (self.stage / name).stat().st_size == description["size"], "fresh backup changed after leader proof")
        return receipt

    def consume_receipt(self):
        require(not os.path.lexists(self.stage / "consumed-receipt.json"), "restore receipt already consumed")
        receipt_path, signature = self.args.receipt, self.args.receipt_signature
        receipt = self.validate_restore_receipt(receipt_path, signature, "backup-request.json")
        request = receipt["request"]
        require(request["preparation"] is None or request["preparation"]["phase"] == "prepared-c",
                 "apply requires prepared C, not an original R authorization")
        require(request.get("cutover") is None or request["cutover"] == {
            "schema": "piping.automation-cutover/v1", "phase": "prepared-c"},
            "apply cannot consume an automation original R proof")
        if self.transforms():
            require("migration-expectation.json" in request["files"], "data expectation is missing from signed receipt")
            self.load_migration_expectation(read_json(self.stage / "fingerprint.json"))
        write(self.stage / "applied-restore-receipt.json", read(receipt_path))
        write(self.stage / "applied-restore-receipt.sig", read(signature))
        write_json(self.stage / "consumed-receipt.json", {"receipt_sha256": file_sha(receipt_path),
                   "request_sha256": receipt["request_sha256"], "nonce": request["nonce"]})

    def install_candidate_config(self):
        self.guard_config()
        self.advance("images-installing")
        replacements = {**self.plan.get("candidate_config", {}), self.plan["image_file"]: "images.next"}
        for name, candidate in replacements.items():
            write(self.root / (name + "." + self.stage.name), read(self.stage / candidate))
        self.guard_config()
        for name in replacements:
            os.replace(self.root / (name + "." + self.stage.name), self.root / name)
        sync_directory(self.root)
        self.guard_config(installed=True)

    def apply(self):
        self.load("backup-verified")
        self.verify_seal("backup-seal.json")
        self.guard_config()
        self.guard_writer_ids()
        self.inspect_images()
        self.stopped()
        self.guard_access_fence()
        self.guard_runtime_inputs()
        self.guard_database()
        baseline = read_json(self.stage / "fingerprint.json")
        require(self.fingerprint() == baseline, "database changed after backup")
        self.guard_target_catalog()
        self.consume_receipt()
        self.advance("restore-starting")
        self.guard_access_fence()
        self.guard_config()
        require(self.fingerprint() == baseline, "target changed during restore receipt consumption")
        self.advance("restore-verified")
        self.advance("apply-starting")
        self.install_candidate_config()
        self.guard_access_fence()
        self.guard_database()
        require(self.fingerprint() == baseline, "database changed before migration")
        self.advance("migrating")
        try:
            self.maintenance_run(self.fenced_target())
            self.guard_access_fence()
            self.guard_database(completed=True)
            self.guard_config(installed=True)
            self.verify_upgrade(baseline)
            self.advance("migration-verified")
            self.advance("starting")
            self.controlled_start()
            self.advance("started-awaiting-health")
        except BaseException:
            self.retain_access_fence()
            raise



class BoundaryParser(argparse.ArgumentParser):
    def error(self, message: str) -> Never:
        # argparse's default errors echo untrusted argument values.
        raise DeployError("invalid deployment arguments")


def parse_args():
    parser = BoundaryParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    prepare_parser = actions.add_parser("prepare")
    verify_parser = actions.add_parser("verify-backup")
    stage_parser = actions.add_parser("stage")
    backup_parser = actions.add_parser("backup")
    cutover_parser = actions.add_parser("capture-cutover-r")
    cutover_accept_parser = actions.add_parser("accept-cutover-r")
    backup_recovery_parser = actions.add_parser("restore-backup")
    backup_start_parser = actions.add_parser("start-backup")
    cutover_recovery_parser = actions.add_parser("restore-cutover-r")
    cutover_start_parser = actions.add_parser("start-cutover-r")
    fhrp_parser = actions.add_parser("prepare-fhrp-auth-keys")
    recovery_parser = actions.add_parser("restore-original-r")
    recovery_start_parser = actions.add_parser("start-original-r")
    abort_parser = actions.add_parser("acknowledge-old-availability")
    apply_parser = actions.add_parser("apply")
    health_parser = actions.add_parser("verify-health")
    start_parser = actions.add_parser("start")
    start_parser.add_argument("--root", required=True, type=Path)
    start_parser.add_argument("--project-name", required=True)
    start_parser.add_argument("--compose-file", default=COMPOSE_FILE_DEFAULT)
    local_parser = actions.add_parser("local-update")
    local_parser.add_argument("--root", required=True, type=Path)
    local_parser.add_argument("--project-name", required=True)
    local_parser.add_argument("--compose-file", default=COMPOSE_FILE_DEFAULT)
    local_parser.add_argument("--image", required=True, action="append")
    local_parser.add_argument("--skip-pull", action="store_true")
    for command in (start_parser, local_parser):
        command.add_argument("--verification-key", type=Path,
                             help="trusted signer key for retained historical recovery inputs")
    activation_parser = actions.add_parser("activate-job-network")
    activation_parser.add_argument("--root", required=True, type=Path)
    activation_parser.add_argument("--project-name", required=True)
    activation_parser.add_argument("--network", required=True)
    activation_parser.add_argument("--bridge", required=True)
    deactivation_parser = actions.add_parser("deactivate-job-network")
    deactivation_parser.add_argument("--root", required=True, type=Path)
    deactivation_parser.add_argument("--project-name", required=True)
    for command in (stage_parser, backup_parser, apply_parser, health_parser, fhrp_parser, recovery_parser,
                    recovery_start_parser, abort_parser, cutover_parser, cutover_accept_parser, backup_recovery_parser,
                    backup_start_parser, cutover_recovery_parser, cutover_start_parser):
        command.add_argument("--root", required=True, type=Path)
        command.add_argument("--stage", required=True, type=Path)
        command.add_argument("--project-name", required=True)
        command.add_argument("--compose-file", default=COMPOSE_FILE_DEFAULT)
        command.add_argument("--proof-subnet", type=lambda value: str(Deployment.private_network(value, prefix=24)))
    verify_parser.add_argument("--proof-subnet", type=lambda value: str(Deployment.private_network(value, prefix=24)))
    for command in (prepare_parser, verify_parser, stage_parser):
        command.add_argument("--package", required=True, type=Path)
    for command in (prepare_parser, verify_parser):
        command.add_argument("--signing-key", required=True, type=Path)
    fhrp_parser.add_argument("--maintenance-subnet", required=True,
                            type=lambda value: str(Deployment.private_network(value, prefix=29)))
    fhrp_parser.add_argument("--receipt", type=Path)
    fhrp_parser.add_argument("--receipt-signature", type=Path)
    backup_parser.add_argument("--maintenance-subnet", type=lambda value: str(Deployment.private_network(value, prefix=29)))
    cutover_parser.add_argument("--maintenance-subnet", type=lambda value: str(Deployment.private_network(value, prefix=29)))
    for command in (cutover_accept_parser, backup_recovery_parser):
        command.add_argument("--receipt", type=Path, required=command is cutover_accept_parser)
        command.add_argument("--receipt-signature", type=Path, required=command is cutover_accept_parser)
    for command in (verify_parser, stage_parser):
        command.add_argument("--verification-key", required=True, type=Path)
    abort_parser.add_argument("--verification-key", required=True, type=Path)
    abort_parser.add_argument("--operator-result", required=True, type=Path)
    abort_parser.add_argument("--operator-result-signature", required=True, type=Path)
    prepare_parser.add_argument("--release", required=True, type=Path)
    prepare_parser.add_argument("--commit", required=True)
    prepare_parser.add_argument("--pipeline-id", required=True)
    prepare_parser.add_argument("--platform", required=True)
    prepare_parser.add_argument("--image", required=True, action="append")
    prepare_parser.add_argument("--preservation-manifest", type=Path)
    prepare_parser.add_argument("--check-capture", type=Path)
    stage_parser.add_argument("--disabled-schedule")
    stage_parser.add_argument("--image-file", required=True, choices=(".env", ".images.env"))
    stage_parser.add_argument("--runner-candidate", type=Path,
                              help="private .env and Compose candidate restricted to sandbox runner adoption")
    verify_parser.add_argument("--dossier", required=True, type=Path)
    apply_parser.add_argument("--receipt", required=True, type=Path)
    apply_parser.add_argument("--receipt-signature", required=True, type=Path)
    return parser.parse_args()



def deadline(_signum, _frame):
    raise DeployError("deployment deadline or interruption; inspect private phase journal")


OWNER_SOURCE_FILES = frozenset({
    "scripts/deploy_release_bundle.py",
    "scripts/ci/postgres_migration_contract.py",
    "scripts/ci/compose_delivery_contract.py",
})


def validated_owner_location(value):
    if not isinstance(value, dict):
        return {}
    source, line = value.get("owner_file"), value.get("owner_line")
    if type(source) is str and source in OWNER_SOURCE_FILES and type(line) is int and 0 < line <= 1000000:
        return {"owner_file": source, "owner_line": line}
    return {}


def safe_owner_location(trace):
    root = Path(__file__).resolve().parent.parent
    location, fallback = {}, {}
    while trace is not None:
        try:
            source = Path(trace.tb_frame.f_code.co_filename).resolve().relative_to(root).as_posix()
            candidate = validated_owner_location({"owner_file": source, "owner_line": trace.tb_lineno})
            if candidate:
                fallback = candidate
                if trace.tb_frame.f_code.co_name not in ("require", "run", "checked_path", "read", "read_json", "file_sha", "isolated_sql"):
                    location = candidate
        except (OSError, ValueError):
            # A frame outside this checkout cannot be an allowed owner.
            pass
        trace = trace.tb_next
    return location or fallback


def record_failure(deployment, reason, error):
    # Deployment boundary reasons contain no command output or argument values.
    # A diagnostic failure must never mask the original fail-closed outcome.
    with contextlib.suppress(OSError, ValueError, KeyError, TypeError):
        phase = deployment.phase()
        write_json(deployment.stage / ("failure-" + uuid.uuid4().hex + ".json"),
                   {"phase": phase, "reason": reason, **safe_owner_location(error.__traceback__),
                    **validated_failure_context(getattr(error, "native_failure_context", None))})


def main():
    global COMMAND_DIAGNOSTICS
    os.umask(0o077)
    signal.signal(signal.SIGALRM, deadline)
    signal.signal(signal.SIGTERM, deadline)
    signal.signal(signal.SIGINT, deadline)
    signal.alarm(45 * 60)
    deployment = None
    try:
        args = parse_args()
        if args.action == "local-update":
            local_update(args)
            return 0
        if args.action == "start":
            start_existing(args)
            return 0
        if args.action == "activate-job-network":
            args.stage = args.root / "backups" / datetime.now(timezone.utc).strftime("release-%Y%m%dT%H%M%SZ")
            deployment = Deployment(args)
            deployment.plan = {"image_file": deployment.installed_image_file(),
                               "compose_file": deployment.compose_path().name}
            COMMAND_DIAGNOSTICS = deployment.root
            with deployment.lock():
                result = deployment.activate_job_network(args.network, args.bridge)
            print("job-network activated runner=" + result["runner"] + " unit=" + result["unit"])
            return 0
        if args.action == "deactivate-job-network":
            args.stage = args.root / "backups" / datetime.now(timezone.utc).strftime("release-%Y%m%dT%H%M%SZ")
            deployment = Deployment(args)
            deployment.plan = {"image_file": deployment.installed_image_file(),
                               "compose_file": deployment.compose_path().name}
            COMMAND_DIAGNOSTICS = deployment.root
            with deployment.lock():
                result = deployment.deactivate_job_network()
            print("job-network deactivated runner=" + result["runner"])
            return 0
        if args.action in ("prepare", "verify-backup"):
            COMMAND_DIAGNOSTICS = args.dossier if args.action == "verify-backup" else args.package
            {"prepare": prepare, "verify-backup": verify_backup}[args.action](args)
            print("leader artifact verified")
            return 0
        deployment = Deployment(args)
        COMMAND_DIAGNOSTICS = deployment.stage
        with deployment.lock():
            {
                "stage": deployment.stage_release,
                "backup": deployment.backup,
                "capture-cutover-r": deployment.capture_cutover_r,
                "accept-cutover-r": deployment.accept_cutover_r,
                "restore-backup": deployment.restore_backup,
                "start-backup": deployment.start_backup,
                "restore-cutover-r": deployment.restore_cutover_r,
                "start-cutover-r": deployment.start_cutover_r,
                "prepare-fhrp-auth-keys": deployment.prepare_fhrp_auth_keys,
                "restore-original-r": deployment.restore_original_r,
                "start-original-r": deployment.start_original_r,
                "acknowledge-old-availability": deployment.acknowledge_old_availability,
                "apply": deployment.apply,
                "verify-health": deployment.verify_runtime_health,
            }[args.action]()
        if args.action == "acknowledge-old-availability":
            print(f"phase={deployment.phase()} upgrade_accepted=false snapshot_historical=true")
        else:
            print(f"phase={deployment.phase()} images=3 migrations={len(deployment.plan['migrations'])}")
    except (
        ValueError,
        OSError,
        KeyError,
        TypeError,
        AttributeError,
        tarfile.TarError,
        subprocess.SubprocessError,
    ) as error:
        if isinstance(error, (RecoveryRequired, ConcurrentOperation)):
            print(str(error), file=sys.stderr)
        if deployment is not None:
            with contextlib.suppress(OSError, ValueError):
                write(deployment.stage / ("boundary-detail-" + uuid.uuid4().hex + ".private.log"), traceback.format_exc().encode())
            record_failure(deployment, str(error) if isinstance(error, DeployError) else "invalid native boundary", error)
        print(
            "deployment refused or interrupted; inspect private phase journal; no replay or automatic rollback",
            file=sys.stderr,
        )
        return 2
    finally:
        signal.alarm(0)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
