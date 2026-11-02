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
CONFIGS = (".env", "docker-compose.yml")
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
                      "starting-original-r", "original-r-started")
ABORTED_OLD_AVAILABILITY = "aborted-old-availability-restored"
PHASE_EDGES = {before: {after} for before, after in zip(PHASES, PHASES[1:])}
PHASE_EDGES["writers-stopped"] = {"access-fenced"}
PHASE_EDGES.update({"access-fenced": {"original-r-dumping", "dumping"},
                   "original-r-dumping": {"original-r-awaiting-proof"},
                   "original-r-awaiting-proof": {"original-r-verified"},
                   "original-r-verified": {"converting-fhrp"},
                   "converting-fhrp": {"prepared-c-verified"}, "prepared-c-verified": {"dumping"}})
for _phase in (*PHASES[PHASES.index("writers-stopped"):], *PREPARATION_PHASES[:-2]):
    PHASE_EDGES.setdefault(_phase, set()).add("recovering-original-r")
PHASE_EDGES["recovering-original-r"] = {"original-r-restored"}
PHASE_EDGES["original-r-restored"] = {"starting-original-r"}
PHASE_EDGES["starting-original-r"] = {"original-r-started"}
PHASE_EDGES["backup-verified"].add(ABORTED_OLD_AVAILABILITY)
MAX_BYTES = 16 * 1024 * 1024
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
    args.image_file = ".images.env" if os.path.lexists(args.root / ".images.env") else ".env"
    args.stage = args.root / "backups" / datetime.now(timezone.utc).strftime("release-%Y%m%dT%H%M%SZ")
    deployment = Deployment(args)
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
                    run(deployment.compose() + ["up", "-d", "--no-deps", "--no-build", "--pull", "never",
                                                "--wait", "--wait-timeout", "180", *services], timeout=210)
                deployment.verify_runtime_health()
            except BaseException:
                run(deployment.compose() + ["stop", "--timeout", "60", *WRITERS])
                raise
        except BaseException as error:
            record_failure(deployment, "local image update interrupted or refused", error)
            raise
        finally:
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
        deployment.plan = {"image_file": ".images.env" if os.path.lexists(args.root / ".images.env") else ".env"}
        services = run(deployment.compose() + ["ps", "--all", "--services"]).decode().splitlines()
        require({"backend", "frontend", "ansible-runner"} <= set(services), "installed application containers are missing")
        run(deployment.compose() + ["start", "--wait", "--wait-timeout", "180", *services], timeout=210)
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
    if preparation is not None:
        if preparation["phase"] == "original-r":
            required_files.difference_update(("offline-pre-upgrade.dump", "fingerprint.json"))
        else:
            required_files.update(("original-r-request.json", "original-r-receipt.json", "original-r-receipt.sig"))
        required_files.add("original-r-manifest.json")
        required_files.update(read_json(directory / "original-r-manifest.json")["files"])
    if (preparation is None or preparation["phase"] == "prepared-c") and pending_transforms(
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
            and inputs["installed"]["settings"] == inputs["candidate"]["settings"]
            and inputs["installed"]["cipher_reference"] == inputs["candidate"]["cipher_reference"]
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
        deployment.original_manifest(authenticated=preparation["phase"] == "prepared-c")
    deployment.approval()
    expected_oracle = {key: plan[key] for key in ("content_type_seed_keys", "source_roundtrip_checks", "restored_target_catalog",
                      "restored_candidate_catalog", "installed_candidate_additions", "installed_candidate_replacements",
                      "installed_candidate_removals")}
    deployment.service_image = lambda role: deployment.plan["postgres_image"] if role == "postgres" else installed
    deployment.reference_proof({"environment": proof_environment}, existing_environment=True)
    require(all(plan[key] == value for key, value in expected_oracle.items()), "client oracle differs from native source")
    preparation_proof = None
    if preparation is not None and preparation["phase"] == "original-r":
        preparation_proof = {"schema": "piping.fhrp-preparation/v1", "semantics": deployment.prove_original_r_request()}
    else:
        baseline = read_json(directory / "fingerprint.json")
        require(baseline["history"] == plan["installed_history"], "backup ledger differs from staged history")
        if deployment.transforms():
            deployment.load_migration_expectation(baseline)
        deployment.restore_proof(baseline)
    # Rehash after proof so a receipt cannot refer to bytes changed during restore.
    for name, description in request["files"].items():
        require(file_sha(directory / name) == description["sha256"], "backup bytes changed during proof")
    receipt = {"schema": "piping.compose-restore-receipt/v1", "request_sha256": file_sha(directory / "backup-request.json"),
               "request": request, "result": "native-restore-migration-conservation-passed"}
    if preparation_proof is not None:
        receipt["preparation_proof"] = preparation_proof
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


# Seal the accepted baseline's layout contract. Forward authority comes only from
# native execution of the SHA-bound source artifact, never image-supplied deltas.
CURRENT_MIGRATION_CONTRACT_SHA256 = "91629d441685bbc09ce55327f9c8d897fc6d13576bd6221249c5713a119797b7"


def validate_contract(contract, migrations):
    require(
        isinstance(contract, dict)
        and set(contract) == {"schema", "installed_prefix", "pending_tail", "deltas", "position_pairs"}
        and contract["schema"] == "piping.compose-migrations/v1"
        and type(contract["installed_prefix"]) is int
        and contract["installed_prefix"] == 1
        and contract["pending_tail"] == []
        and contract["deltas"] == []
        and isinstance(migrations, dict)
        # Canonical stage JSON sorts object keys lexically, not by migration number.
        and set(migrations) == {str(v) for v in range(1, len(migrations) + 1)}
        and migrations.get("1") == BASELINE_SQLX_SHA384
        and all(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{96}", value)
                for value in migrations.values())
        and sha(canonical(contract)) == CURRENT_MIGRATION_CONTRACT_SHA256,
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
    fresh, installed = columns(canonical_catalog), columns(installed_catalog)
    require(all(fresh.get(name) == pair[0] for name, pair in pairs.items()),
            "source baseline layout differs from sealed native proof")
    if all(installed.get(name) == pair[0] for name, pair in pairs.items()):
        return {}
    require(all(installed.get(name) == pair[1] for name, pair in pairs.items()),
            "unrecognized native baseline column layout")
    return pairs



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


class Deployment:
    def __init__(self, args):
        self.args, self.root, self.stage = args, args.root, args.stage
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
        self.plan: dict[str, Any] = {}

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

    def compose(self, images=None):
        image_file = self.plan["image_file"]
        env_files = [str(images or self.root / ".env")] if image_file == ".env" else [
            str(self.root / ".env"), str(images or self.root / image_file)
        ]
        return [
            "docker",
            "compose",
            "--project-name",
            self.args.project_name,
            "--project-directory",
            str(self.root),
            *(part for name in env_files for part in ("--env-file", name)),
            "-f",
            str(self.root / "docker-compose.yml"),
            "--profile",
            "*",
        ]

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

    def guard_recovery(self):
        backups = self.root / "backups"
        if not os.path.lexists(backups):
            return
        checked_path(backups, directory=True)
        # A validated recovery checkpoint closes earlier operation authority.
        # Inspect every newer operation before accepting that boundary.
        for stage in sorted(backups.iterdir(), reverse=True):
            if not stage.name.startswith("release-"):
                continue
            owner = Deployment(argparse.Namespace(root=self.root, stage=stage, project_name=self.args.project_name))
            phase = owner.phase()
            if phase == ABORTED_OLD_AVAILABILITY:
                owner.validate_abort_history()
                return
            if phase not in ("staging", "staged", "backup-starting", "healthy", "original-r-started"):
                raise RecoveryRequired("Unresolved update journal; inspect backups and recover explicitly before starting or updating")

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
            expected_code_sha256=value["code_sha256"], expected_request_sha256=value["request_sha256"]), fence

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
            "layout_sha256": sha(canonical(self.plan["installed_append_slots"])),
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
                and observation["layout_sha256"] == sha(canonical(self.plan["installed_append_slots"]))
                and observation["check_bindings_sha256"] == sha(canonical(self.plan["installed_check_bindings"]))
                and all(writer["running"] is self.plan["writer_running"][role]
                        for role, writer in observation["writers"].items()), "abort observation binding changed")
        return compose_abort_acknowledgment(raw, expected_bindings=self.abort_bindings(result,
            acknowledging_helper=acknowledgment["acknowledging_helper_sha256"], observation=observation,
            observed_at=acknowledgment["observed_at"]))

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
            compose_abort_acknowledgment(canonical(acknowledgment), expected_bindings=bindings)
            write_json(self.stage / names[2], acknowledgment)
            self.validate_abort_history()
        self.advance(ABORTED_OLD_AVAILABILITY)

    def guard_latest_stage(self):
        stages = [path.name for path in (self.root / "backups").iterdir()
                  if re.fullmatch(r"release-[0-9]{8}T[0-9]{6}Z", path.name)]
        require(stages and max(stages) == self.stage.name, "a newer stage supersedes this operation")

    def guard_config(self, *, installed=False):
        for name, expected in self.plan["baseline"].items():
            if installed and name == self.plan["image_file"]:
                expected = file_sha(self.stage / "images.next")
            require(
                file_sha(self.root / name) == expected,
                "configuration changed since staging",
            )

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
        config_names = list(CONFIGS)
        if args.image_file == ".images.env" or os.path.lexists(self.root / ".images.env"):
            config_names.append(".images.env")
        originals = {name: read(self.root / name) for name in config_names}
        candidate = render_images(originals[args.image_file], {role: image["local_id"] for role, image in images.items()})
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
        }
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

    def target_additions(self, slots):
        additions = json.loads(json.dumps(self.pending_delta()["additions"]))
        first = {}
        for obj in additions:
            if obj["kind"] == "column":
                table = obj["name"].rsplit(".", 1)[0]
                first[table] = min(first.get(table, obj["definition"]["position"]), obj["definition"]["position"])
        for obj in additions:
            if obj["kind"] == "column":
                table = obj["name"].rsplit(".", 1)[0]
                if table in slots:
                    obj["definition"]["position"] += slots[table] + 1 - first[table]
        return additions

    def pending_delta(self):
        require("source_delta" in self.plan, "native source migration delta is not proven")
        return self.plan["source_delta"]

    def service_cutover(self):
        return service_cutover_pending(self.plan.get("prefix_manifest_sha384", {}),
                                       len(self.plan.get("installed_history", ())))

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
        untouched = set(baseline["rows"]) - changed
        require(set(projected["rows"]) == set(baseline["rows"]) - removed
                and all(projected["rows"][name] == baseline["rows"][name] for name in untouched),
                "native data expectation omitted or changed unrelated rows")
        require(self.fingerprint() == expected_raw, "source changed during native data expectation")
        return {"schema": "piping.migration-conservation/v1",
                "context": self.expectation_context(baseline),
                "columns": projected["columns"], "rows": projected["rows"], "content_type_seeds": content_types}

    def load_migration_expectation(self, baseline):
        value = read_json(self.stage / "migration-expectation.json")
        changed = {*SERVICE_TABLES, SERVICE_MAPPING_TABLE} if self.service_cutover() else set()
        if "FHRP0004" in self.transforms():
            changed.add(FHRP_TABLE)
        if "Tag0006" in self.transforms():
            changed.update((*tag_cutover_fields(baseline["columns"]), TAG_CATALOG_TABLE))
        require(isinstance(value, dict) and set(value) == {"schema", "context", "columns", "rows", "content_type_seeds"}
                and value["schema"] == "piping.migration-conservation/v1"
                and value["context"] == self.expectation_context(baseline)
                and value["columns"] == transformation_columns(baseline["columns"], self.transforms())
                and set(value["rows"]) == set(baseline["rows"]) - (
                    {SERVICE_MAPPING_TABLE} if self.service_cutover() else set())
                and all(value["rows"][name] == baseline["rows"][name]
                        for name in set(baseline["rows"]) - changed),
                "data transformation expectation is not bound to the raw backup")
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

    def source_readiness(self, canonical_catalog, database):
        approval = self.approval()
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
        else:
            with self.database_target(database):
                require(self.append_slots() == self.plan["installed_append_slots"],
                        "canonical physical append slots changed without approval")
        positions = (approval["position_pairs"] if approval else baseline_positions(
            self.plan["contract"], canonical_catalog, self.plan["installed_target_catalog"]))
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
                            "interval": "1s", "timeout": "3s", "retries": 60},
        }}})
        compose = ["docker", "compose", "-p", project, "-f", str(config)]
        try:
            run(compose + ["up", "-d", "--wait", "--wait-timeout", "90", "--pull", "never", "--no-build"], timeout=120)
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
                source_history = self.guard_history(preserve_installed=False)
                require(len(source_history) == installed_version, "source reference is not the installed prefix")
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
            self.source_readiness(source_catalog, database)
            self.plan["source_roundtrip_checks"] = self.source_roundtrip_checks(database, source_catalog)
            if pending:
                self.isolated_migrate(database, self.image("backend"))
                with self.database_target(database):
                    exact_target_preservation(source_catalog, self.catalog(), **delta)
                    candidate_history = self.guard_history(completed=True, preserve_installed=False)
                    require(candidate_history[:installed_version] == source_history,
                            "source migrator rewrote installed SQLx metadata")
        # A separate target-native oracle starts from the actual private schema
        # snapshot, never from a factory catalog or a client diagnostic receipt.
        with self.isolated_database("installed-target-reference") as database:
            self.restore_archive(database, self.stage / "installed-target-schema.dump")
            with self.database_target(database):
                restored = self.roundtrip_target(database)
                self.seed_reference_history()
                restored_slots = self.append_slots()
            self.plan["restored_target_catalog"] = restored
            additions = self.target_additions(restored_slots)
            self.isolated_migrate(database, self.image("backend"))
            with self.database_target(database):
                candidate = self.catalog()
                exact_target_preservation(restored, candidate, additions=additions,
                                          replacements=self.pending_delta()["replacements"],
                                          removals=self.pending_delta()["removals"])
                self.guard_history(completed=True)
            self.plan["restored_candidate_catalog"] = candidate
        self.plan["installed_candidate_additions"] = self.target_additions(self.plan["installed_append_slots"])
        self.plan["installed_candidate_replacements"] = self.pending_delta()["replacements"]
        self.plan["installed_candidate_removals"] = self.pending_delta()["removals"]

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
            after = {(o["kind"], o["name"]): o["definition"] for o in self.catalog()}
        return {o["name"]: [o["definition"], after[(o["kind"], o["name"])]]
                for o in source_catalog if o["kind"] == "constraint" and o["definition"].startswith("CHECK ")}

    def restore_archive(self, database, archive):
        with contextlib.ExitStack() as stack:
            source = archive if hasattr(archive, "read") else stack.enter_context(checked_path(archive).open("rb"))
            output = stack.enter_context(tempfile.TemporaryFile())
            result = subprocess.run(database["compose"] + ["exec", "-T", "postgres", "pg_restore",
                "--exit-on-error", "--no-owner", "--no-privileges", "-U", "piping", "-d", database.get("name", "piping")],
                stdin=source, stdout=output, stderr=output, timeout=300, env=environment(), check=False)
            if result.returncode:
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
            self.original_manifest()
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

    def fingerprint(self, columns=None, *, project_transforms=False, content_type_ids=None):
        catalog = self.catalog()
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
            require(self.transforms() and not projecting_old_columns,
                    "native projection requires the exact pending data transformation")
            columns = transformation_columns(columns, self.transforms())
        added_tables = {o["name"] for o in self.plan.get("source_delta", {}).get("additions", [])
                        if o["kind"] == "relation" and o["definition"]["kind"] in ("r", "p", "v", "f")}
        require(set(columns) <= set(live_columns)
                and set(live_columns) - set(columns) <= added_tables | (
                    {SERVICE_MAPPING_TABLE} if project_transforms and self.service_cutover() else set()),
                "database table coverage changed outside native source delta")
        statements = []
        for table, names in columns.items():
            computed = {"port_mappings"} if project_transforms and self.service_cutover() and table in SERVICE_TABLES else set()
            require(set(names) - computed <= set(live_columns[table]), "old database column missing")
            identifier = ".".join('"' + part.replace('"', '""') + '"' for part in table.split("."))
            fields = ",".join(service_cutover_expression(table) if name in computed
                              else 'source."' + name.replace('"', '""') + '"' for name in names)
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

    def verify_conservation(self, baseline, *, restored=False):
        expected = self.load_migration_expectation(baseline) if self.transforms() else baseline
        content_types = expected.get("content_type_seeds")
        actual = self.fingerprint(expected["columns"], content_type_ids=(
            content_types["original_ids"] if content_types is not None else None))
        if restored:
            exact_target_preservation(self.plan["restored_candidate_catalog"], actual["catalog"])
        else:
            exact_target_preservation(self.plan["installed_target_catalog"], actual["catalog"],
                                      additions=self.plan["installed_candidate_additions"],
                                      replacements=self.plan["installed_candidate_replacements"],
                                      removals=self.plan["installed_candidate_removals"])
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
        if len(baseline["history"]) < len(self.plan["migrations"]):
            for obj in self.pending_delta()["additions"]:
                if obj["kind"] == "column" and obj["name"].rsplit(".", 1)[0] in baseline["columns"]:
                    if self.service_cutover() and obj["name"] in {table + ".port_mappings" for table in SERVICE_TABLES}:
                        # These values are checked against every pre-upgrade row,
                        # not mistaken for a default-valued additive column.
                        continue
                    _, table, column = obj["name"].split(".")
                    require(self.sql('SELECT NOT EXISTS (SELECT 1 FROM public."' + table + '" WHERE "'
                                      + column + '" IS DISTINCT FROM ' + (obj["definition"]["default"] or "NULL") + ');') == b"t",
                            "old rows acquired non-default migration values")

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
                "cipher_reference": cipher_ref, "executables": digests}

    def image_inputs(self, image, *, converter):
        container = run(["docker", "create", "--label", "piping.release-stage=" + self.stage.name,
                         "--entrypoint", "/bin/true", image]).decode().strip()
        try:
            require(re.fullmatch(r"[0-9a-f]{64}", container), "invalid input-inspection container")
            return self.container_inputs(container, converter=converter)
        finally:
            run(["docker", "rm", container])

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
        require(live == old and old["settings"] == candidate["settings"]
                and old["cipher_reference"] == candidate["cipher_reference"],
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
        return {"schema": "piping.deployment-runtime-inputs/v1", "installed": old, "candidate": candidate,
                "environment_sha256": file_sha(self.stage / "proof-backend.env"),
                "effective_cipher_reference": cipher_ref, "effective_cipher_environment_sha256": effective_hash}

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
        self.original_manifest(authenticated=False)
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
            self.original_manifest()
            recovery = read_json(self.stage / "recovery-images.json")
            allowed.add(recovery[role]["image_id"])
        require(labels.get("piping.preparation-start") == fence["nonce"]
                    and labels.get("com.docker.compose.project") == self.args.project_name
                    and labels.get("com.docker.compose.service") == service and inspected["Image"] in allowed,
                    "unrecognized replacement writer container")
        return identifier

    def retain_access_fence(self, *, allow_missing=False):
        """Failure path: close access first, then stop only positively owned clients."""
        self.guard_latest_stage()
        if os.path.lexists(self.stage / "original-r-manifest.json"):
            self.original_manifest()
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
        command = self.compose() + ["-f", str(self.stage / "startup.compose.json")]
        model = json.loads(run(command + ["config", "--format", "json"]))
        for role in writers:
            base["services"][role].setdefault("labels", {})["piping.preparation-start"] = fence["nonce"]
        require(base == model, "startup overlay changes more than ownership labels")
        self.replace_hba(fence["postgres"], read(self.stage / "original-pg_hba.conf"), uuid.uuid4().hex)
        require(self.hba_bytes(fence["postgres"]) == read(self.stage / "original-pg_hba.conf"),
                "original native access rules were not restored")
        try:
            run(command + ["up", "-d", "--wait", "--wait-timeout", "180", "--no-deps", "--pull", "never",
                           "--no-build", *writers, "frontend"], timeout=210)
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
        phase = self.phase()
        require(phase != "original-r-restored" and "recovering-original-r" in PHASE_EDGES.get(phase, ()),
                "original R recovery is unavailable or already attempted")
        self.load(phase)
        self.original_manifest()
        # Configuration may only be exactly staged-original or exactly the
        # candidate image assignment; unknown substitutions are never repaired.
        for name, digest in self.plan["baseline"].items():
            allowed = {digest}
            if name == self.plan["image_file"]:
                allowed.add(file_sha(self.stage / "images.next"))
            require(file_sha(self.root / name) in allowed, "configuration substitution blocks original R recovery")
        self.resume_contained_provider()
        self.retain_access_fence(allow_missing=True)
        fence = self.guard_access_fence(allow_missing_writers=True)
        self.guard_recovery_capacity(max(self.original_manifest()["database_bytes"],
                                        int(self.sql("SELECT pg_database_size(current_database());"))))
        images = read_json(self.stage / "recovery-images.json")
        require(set(images) == set(ROLES), "incomplete original image set")
        for role, image in images.items():
            loaded = load_image(self.stage, image["archive"], self.plan["platform"])
            if "@" not in image["reference"] and not image["reference"].startswith("sha256:"):
                run(["docker", "image", "tag", loaded, image["reference"]])
            inspected = json.loads(run(["docker", "image", "inspect", image["reference"]]))[0]
            require(inspected["Id"] == image["image_id"], "original image reference was not restored exactly")
        original = read_json(self.stage / "original-r-fingerprint.json")
        restored_name = "piping_recovery_" + fence["nonce"]
        quarantine = "piping_quarantine_" + fence["nonce"]
        require(self.sql("SELECT NOT EXISTS (SELECT FROM pg_database WHERE datname IN ('" + restored_name
                         + "','" + quarantine + "'));") == b"t", "recovery/quarantine identity already exists")
        self.advance("recovering-original-r")
        self.sql("CREATE DATABASE " + restored_name + ";")
        database = {"compose": self.compose(), "name": restored_name}
        self.raw_restore(database, self.stage / "original-r.dump", original)
        self.original_manifest()
        self.guard_access_fence(allow_missing_writers=True)
        # The failed target remains retained under an operation-bound identity.
        run(self.postgres_command("psql", "-X", "-qAt", "-v", "ON_ERROR_STOP=1", "-U", "piping", "-d", "postgres"),
            data=("BEGIN; ALTER DATABASE piping ALLOW_CONNECTIONS false; ALTER DATABASE piping RENAME TO " + quarantine
                  + "; ALTER DATABASE " + restored_name + " RENAME TO piping; COMMIT;").encode())
        for name in self.plan["baseline"]:
            temporary = self.root / (name + ".recovery-" + fence["nonce"])
            write(temporary, read(self.stage / (name + ".original")))
            os.replace(temporary, self.root / name)
        sync_directory(self.root)
        self.guard_config()
        require(self.fingerprint() == {**original, "catalog": self.plan["restored_target_catalog"]},
                "recovered original R changed during database switch")
        self.guard_access_fence(allow_missing_writers=True)
        write_json(self.stage / "original-r-recovery.json", {"original_manifest_sha256": file_sha(self.stage / "original-r-manifest.json"),
                   "quarantine": quarantine, "database": "piping", "writers_stopped": True})
        self.advance("original-r-restored")

    def start_original_r(self):
        self.load("original-r-restored")
        self.original_manifest()
        self.guard_config()
        self.guard_access_fence(allow_missing_writers=True)
        original = read_json(self.stage / "original-r-fingerprint.json")
        require(self.fingerprint() == {**original, "catalog": self.plan["restored_target_catalog"]},
                "restored original R changed before explicit startup")
        self.advance("starting-original-r")
        try:
            self.controlled_start(original=True)
            self.runtime_health({role: image["image_id"] for role, image in
                                 read_json(self.stage / "recovery-images.json").items()})
            self.guard_database()
            self.advance("original-r-started")
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

    def original_manifest(self, *, authenticated=True):
        if authenticated:
            receipt = self.validate_restore_receipt(self.stage / "original-r-receipt.json",
                self.stage / "original-r-receipt.sig", "original-r-request.json")
            require(receipt["request"]["preparation"] == {
                "schema": "piping.fhrp-preparation/v1", "phase": "original-r",
                "original_manifest_sha256": file_sha(self.stage / "original-r-manifest.json")},
                "original R receipt is not a preparation authorization")
        value = read_json(self.stage / "original-r-manifest.json")
        require(set(value) == {"schema", "plan_sha256", "helper_sha256", "database_bytes", "files"}
                and value["schema"] == "piping.fhrp-original-r/v1"
                and value["plan_sha256"] == file_sha(self.stage / "plan.json")
                and value["helper_sha256"] == helper_identity()
                and type(value["database_bytes"]) is int and value["database_bytes"] > 0,
                "original R authentication context mismatch")
        expected = {"original-r.dump", "original-r-fingerprint.json", "original-r-source.json",
                    "runtime-inputs.json", "access-fence.json", "original-pg_hba.conf", "proof-backend.env",
                    "startup.compose.json", "writers.json", "recovery-images.json", "installed-target-schema.dump",
                    *(name + ".original" for name in self.plan["baseline"])}
        recovery_images = read_json(self.stage / "recovery-images.json")
        require(set(recovery_images) == set(ROLES), "incomplete authenticated recovery image set")
        for role, image in recovery_images.items():
            require(role in ROLES and set(image) == {"image_id", "reference", "archive"}, "invalid recovery image record")
            require(image["archive"]["archive"] == "recovery-" + role + ".tar", "invalid recovery archive identity")
            expected.add(image["archive"]["archive"])
        require(set(value["files"]) == expected, "original R manifest member set mismatch")
        for name, digest in value["files"].items():
            require(isinstance(name, str) and Path(name).name == name and file_sha(self.stage / name) == digest,
                    "original R member identity mismatch")
        require(all(file_sha(self.stage / (name + ".original")) == digest for name, digest in self.plan["baseline"].items()),
                "original configuration bytes differ from their staged identities")
        self.controlled_writers()
        return value

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
        self.original_manifest()
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
        self.original_manifest()
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
        files = {"original-r.dump", "original-r-fingerprint.json", "original-r-source.json", "runtime-inputs.json",
                 "access-fence.json", "original-pg_hba.conf", "proof-backend.env", "startup.compose.json",
                 "writers.json", "recovery-images.json", "installed-target-schema.dump",
                 *(name + ".original" for name in self.plan["baseline"])}
        files.update(image["archive"]["archive"] for image in read_json(self.stage / "recovery-images.json").values())
        with self.isolated_database("original-r-proof") as database:
            self.raw_restore(database, self.stage / "original-r.dump", original)
            write_json(self.stage / "original-r-manifest.json", {
                "schema": "piping.fhrp-original-r/v1", "plan_sha256": file_sha(self.stage / "plan.json"),
                "database_bytes": size,
                "helper_sha256": helper_identity(), "files": {name: file_sha(self.stage / name) for name in sorted(files)}})
            self.original_manifest(authenticated=False)
        self.guard_access_fence()
        require(self.fingerprint() == original, "original source changed during local restore proof")
        self.write_backup_request(size, postgres_archive, installed_archive,
            ["original-r-manifest.json", *sorted(files)], preparation_phase="original-r")
        write(self.stage / "original-r-request.json", read(self.stage / "backup-request.json"))
        self.seal("original-r-seal.json", ["backup-request.json", "original-r-request.json", "writers.json",
                  "original-r-manifest.json", *sorted(files), postgres_archive["archive"], installed_archive["archive"]])
        self.advance("original-r-awaiting-proof")

    def backup(self, *, prepare_original=False):
        self.load("staged")
        self.guard_config()
        self.guard_writer_ids()
        self.inspect_images()
        self.guard_database()
        self.guard_target_catalog()
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
        if prepare_original:
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
        self.complete_backup(size, postgres_archive, installed_archive)

    def complete_backup(self, size, postgres_archive, installed_archive, *, prepare_original=False):
        self.guard_database()
        self.guard_config()
        self.guard_target_catalog()
        fingerprint = self.fingerprint()
        transformation_files = ["runtime-inputs.json", "access-fence.json", "original-pg_hba.conf", "startup.compose.json", "writers.json"]
        if prepare_original:
            original = self.original_manifest()
            transformation_files.extend(["original-r-manifest.json", "original-r-request.json",
                "original-r-receipt.json", "original-r-receipt.sig", *sorted(original["files"])])
            transformation_files = sorted(set(transformation_files))
        if self.transforms():
            write_json(self.stage / "migration-expectation.json", self.migration_expectation(fingerprint))
            transformation_files.append("migration-expectation.json")
        self.advance("dumping")
        require(self.raw_archive("offline-pre-upgrade.dump", "fingerprint.json") == fingerprint,
                "database changed after native expectation")
        self.guard_database()
        self.stopped()
        self.guard_config()
        self.write_backup_request(size, postgres_archive, installed_archive,
            ["offline-pre-upgrade.dump", "fingerprint.json", "proof-backend.env", "installed-target-schema.dump",
             *transformation_files], preparation_phase="prepared-c" if prepare_original else None)
        self.seal("backup-seal.json", ["offline-pre-upgrade.dump", "fingerprint.json", "writers.json", "backup-request.json",
                  *transformation_files, postgres_archive["archive"], *([installed_archive["archive"]] if installed_archive else [])])
        self.advance("backup-verified")

    def write_backup_request(self, size, postgres_archive, installed_archive, files, *, preparation_phase=None):
        require(preparation_phase in (None, "original-r", "prepared-c"), "unsupported preparation request phase")
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
                   "files": {name: {"sha256": file_sha(self.stage / name), "size": (self.stage / name).stat().st_size}
                              for name in (*files,
                                           *(["preservation.json"] if self.plan["preservation_sha256"] else []),
                                           *(["check-capture.json"] if self.plan["check_capture_sha256"] else []))}}
        if preparation_phase == "prepared-c":
            write_json(self.stage / "prepared-c-request.json", request)
            os.replace(self.stage / "prepared-c-request.json", self.stage / "backup-request.json")
            sync_directory(self.stage)
        else:
            write_json(self.stage / "backup-request.json", request)

    def validate_restore_receipt(self, receipt_path, signature, request_name):
        authenticate(receipt_path, signature, getattr(self, "verifier_key", self.stage / "verification-key.pem"))
        receipt = read_json(receipt_path)
        request = read_json(self.stage / request_name)
        require(receipt["schema"] == "piping.compose-restore-receipt/v1"
                and receipt["result"] == "native-restore-migration-conservation-passed"
                and receipt["request_sha256"] == file_sha(self.stage / request_name)
                and receipt["request"] == request
                and request["helper_sha256"] == helper_identity()
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
        if self.transforms():
            require("migration-expectation.json" in request["files"], "data expectation is missing from signed receipt")
            self.load_migration_expectation(read_json(self.stage / "fingerprint.json"))
        write(self.stage / "applied-restore-receipt.json", read(receipt_path))
        write(self.stage / "applied-restore-receipt.sig", read(signature))
        write_json(self.stage / "consumed-receipt.json", {"receipt_sha256": file_sha(receipt_path),
                   "request_sha256": receipt["request_sha256"], "nonce": request["nonce"]})

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
        temporary = self.root / (self.plan["image_file"] + "." + self.stage.name)
        self.advance("images-installing")
        write(temporary, read(self.stage / "images.next"))
        self.guard_config()
        os.replace(temporary, self.root / self.plan["image_file"])
        sync_directory(self.root)
        self.guard_config(installed=True)
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
    fhrp_parser = actions.add_parser("prepare-fhrp-auth-keys")
    recovery_parser = actions.add_parser("restore-original-r")
    recovery_start_parser = actions.add_parser("start-original-r")
    abort_parser = actions.add_parser("acknowledge-old-availability")
    apply_parser = actions.add_parser("apply")
    health_parser = actions.add_parser("verify-health")
    start_parser = actions.add_parser("start")
    start_parser.add_argument("--root", required=True, type=Path)
    start_parser.add_argument("--project-name", required=True)
    local_parser = actions.add_parser("local-update")
    local_parser.add_argument("--root", required=True, type=Path)
    local_parser.add_argument("--project-name", required=True)
    local_parser.add_argument("--image", required=True, action="append")
    local_parser.add_argument("--skip-pull", action="store_true")
    for command in (stage_parser, backup_parser, apply_parser, health_parser, fhrp_parser, recovery_parser,
                    recovery_start_parser, abort_parser):
        command.add_argument("--root", required=True, type=Path)
        command.add_argument("--stage", required=True, type=Path)
        command.add_argument("--project-name", required=True)
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
