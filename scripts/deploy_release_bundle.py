#!/usr/bin/env python3
"""Guarded Compose updates from published bundles or locally selected images."""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import io
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
    CATALOG_SQL, CHECK_BINDINGS_SQL, catalog_objects, canonical_readiness,
    definition_hash, exact_target_preservation, historical_check_forms, migration_catalog,
    preservation_extensions, prove_native_check, prove_original_native_checks,
)
from scripts.ci.compose_delivery_contract import (
    canonical, package_binding, preservation_manifest, save_authority, save_identity, verify_manifest,
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
        except BaseException:
            record_failure(deployment, "local image update interrupted or refused")
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
    required_files = {"offline-pre-upgrade.dump", "fingerprint.json", "proof-backend.env", "installed-target-schema.dump"}
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
    deployment.args = argparse.Namespace(project_name="leader-proof-" + request["nonce"])
    deployment.root, deployment.stage, deployment.plan = directory, directory, plan
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
    proof_environment = {}
    for line in read(directory / "proof-backend.env").decode().splitlines():
        key, value = line.split("=", 1)
        require(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key) and key not in proof_environment
                and key not in {"PIPING_MIGRATIONS_DIR", "LD_PRELOAD", "LD_LIBRARY_PATH"},
                "unsafe proof environment")
        proof_environment[key] = value
    deployment.approval()
    expected_oracle = {key: plan[key] for key in ("source_roundtrip_checks", "restored_target_catalog",
                      "restored_candidate_catalog", "installed_candidate_additions", "installed_candidate_replacements")}
    deployment.service_image = lambda role: deployment.plan["postgres_image"] if role == "postgres" else installed
    deployment.reference_proof({"environment": proof_environment}, existing_environment=True)
    require(all(plan[key] == value for key, value in expected_oracle.items()), "client oracle differs from native source")
    baseline = read_json(directory / "fingerprint.json")
    require(baseline["history"] == plan["installed_history"], "backup ledger differs from staged history")
    deployment.restore_proof(baseline)
    # Rehash after proof so a receipt cannot refer to bytes changed during restore.
    for name, description in request["files"].items():
        require(file_sha(directory / name) == description["sha256"], "backup bytes changed during proof")
    receipt = {"schema": "piping.compose-restore-receipt/v1", "request_sha256": file_sha(directory / "backup-request.json"),
               "request": request, "result": "native-restore-migration-conservation-passed"}
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


def run(command, *, data=None, timeout=120, output=None, maximum=MAX_BYTES):
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
        if result.returncode and COMMAND_DIAGNOSTICS is not None:
            # Keep native stderr private; never place it in public exceptions.
            with contextlib.suppress(OSError, ValueError):
                checked_path(COMMAND_DIAGNOSTICS, directory=True)
                require(stat.S_IMODE(COMMAND_DIAGNOSTICS.stat().st_mode) == 0o700,
                        "command diagnostics require a private stage")
                errors.seek(0)
                write(COMMAND_DIAGNOSTICS / ("native-command-" + uuid.uuid4().hex + ".log"),
                      errors.read(MAX_BYTES))
        require(result.returncode == 0, "native command failed; private output suppressed")
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
            match = re.fullmatch(r"([0-9]+)_[A-Za-z0-9_]+\.sql", path.name)
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


# Seal the one current source-owned JSON contract. This helper is carried alone;
# an artifact-supplied JSON document cannot authorize additional schema changes.
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
        and set(migrations) == {"1"}
        and sha(canonical(contract)) == CURRENT_MIGRATION_CONTRACT_SHA256,
        "unsupported source-bound migration contract",
    )
    return contract


def pending_delta(contract, installed_version):
    require(installed_version == 1, "pre-1.0 history requires explicit offline maintenance before deployment")
    return {
        key: [obj for delta in contract["deltas"] if delta["version"] > installed_version for obj in delta[key]]
        for key in ("additions", "replacements")
    }


def expected_catalog(prefix, contract, installed_version=1):
    return migration_catalog(prefix, **pending_delta(contract, installed_version))


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
        require(names and len(names) <= len(PHASES), "invalid phase journal")
        for index, name in enumerate(names):
            require(
                name == f"{index:02d}-{PHASES[index]}" and read(self.stage / "journal" / name) == b"\n",
                "invalid phase journal",
            )
            require(
                stat.S_IMODE((self.stage / "journal" / name).stat().st_mode) == 0o600,
                "phase journal must be private",
            )
        return PHASES[len(names) - 1]

    def advance(self, phase):
        index = PHASES.index(phase)
        if index:
            require(self.phase() == PHASES[index - 1], "phase cannot be replayed")
        write(self.stage / "journal" / f"{index:02d}-{phase}", b"\n")

    def guard_recovery(self):
        backups = self.root / "backups"
        if not os.path.lexists(backups):
            return
        checked_path(backups, directory=True)
        for stage in sorted(backups.iterdir()):
            if not stage.name.startswith("release-"):
                continue
            owner = Deployment(argparse.Namespace(root=self.root, stage=stage, project_name=self.args.project_name))
            phase = owner.phase()
            if PHASES.index(phase) >= PHASES.index("writers-stopping") and phase != "healthy":
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
        """Accept a completed apply only after observing its actual candidate runtime."""
        self.load("started-awaiting-health")
        self.guard_config(installed=True)
        self.inspect_images()
        for role in ROLES:
            require(self.service_image(role) == self.image(role), "runtime is not the applied image selection")
            identifier = run(self.compose() + ["ps", "-aq", role]).decode().strip()
            state = json.loads(run(["docker", "inspect", identifier]))[0]["State"]
            require(state["Running"] and state.get("Health", {}).get("Status", "healthy") == "healthy",
                    "applied runtime is not healthy")
        run(self.compose() + ["exec", "-T", "backend", "curl", "--fail", "--silent",
                               "--max-time", "10", "http://127.0.0.1:7000/api/v1/ready"], timeout=20)
        self.refresh_edge_upstreams()
        self.guard_database(completed=True)
        self.advance("healthy")

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

    def load(self, phase):
        require(self.phase() == phase, "interrupted or completed phase cannot be replayed")
        self.verify_seal("stage-seal.json")
        self.plan = read_json(self.stage / "plan.json")
        require(socket.gethostname() == self.plan["target"], "executing host differs from staged target")
        require(
            self.plan["root"] == str(self.root) and self.plan["project_name"] == self.args.project_name,
            "stage ownership mismatch",
        )
        require(self.plan["image_file"] in (".env", ".images.env"), "invalid sealed image file")
        require(self.plan["helper_sha256"] == helper_identity(), "helper changed after staging")
        authenticate(self.stage / "package.json", self.stage / "package.sig", self.stage / "verification-key.pem")
        require(file_sha(self.stage / "package.json") == self.plan["package_sha256"]
                and file_sha(self.stage / "verification-key.pem") == self.plan["verification_key_sha256"],
                "stage authentication identity changed")
        package = read_json(self.stage / "package.json")
        require(package_binding(package) == self.plan["package_binding_sha256"]
                and package.get("preservation", {}).get("manifest_sha256") == self.plan["preservation_sha256"]
                and package.get("preservation", {}).get("capture_sha256") == self.plan["check_capture_sha256"],
                "preservation authority differs from signed package")
        self.approval()
        require(getattr(self.args, "image_file", None) in (None, self.plan["image_file"]),
                "image assignment file differs from sealed stage")
        validate_contract(self.plan["contract"], self.plan["migrations"])

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
            require(self.plan.get("prefix_manifest_sha384", prefix) == prefix, "published source prefix paths changed")
            self.plan["prefix_manifest_sha384"] = prefix
            return migration_artifact(archive)
        finally:
            run(["docker", "rm", name])

    def stage_release(self):
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
        for service in WRITERS:
            if service in after["services"]:
                ids = run(self.compose() + ["ps", "-aq", service]).decode().splitlines()
                require(len(ids) <= 1, "ambiguous installed writer identity")
                if ids:
                    self.plan["writer_ids"][service] = ids[0]
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
        self.approval()
        snapshot = self.stage / "installed-target-schema.dump"
        checked_path(snapshot, missing=True)
        with snapshot.open("xb") as stream:
            os.chmod(snapshot, 0o600)
            run(self.postgres_command("sh", "-c", 'exec pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" --schema-only --format=custom'),
                output=stream, timeout=300)
            stream.flush()
            os.fsync(stream.fileno())
        self.plan["schema_snapshot_sha256"] = file_sha(snapshot)
        self.guard_target_catalog()
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

    def sql(self, query):
        return run(
            self.postgres_command(
                "sh",
                "-c",
                'exec psql -X -qAt -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB"',
            ),
            data=query.encode(),
        ).strip()

    def history(self):
        return json.loads(self.sql(
            "SELECT coalesce(json_agg(row_to_json(h) ORDER BY version::bigint),'[]'::json) FROM "
            "(SELECT version::text,description,installed_on AT TIME ZONE 'UTC' AS installed_on,success,encode(checksum,'hex') AS checksum,"
            "execution_time FROM public._sqlx_migrations) h;"
        ))

    def guard_history(self, *, completed=False):
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
        baseline = self.plan.get("installed_history")
        if baseline is not None:
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
        tables = {o["name"].rsplit(".", 1)[0] for delta in self.plan["contract"]["deltas"] for o in delta["additions"] if o["kind"] == "column"}
        if not tables:
            return {}
        literal = lambda value: "'" + value.replace("'", "''") + "'"
        return json.loads(self.sql("SELECT json_object_agg(name,slot) FROM (SELECT n.nspname||'.'||c.relname AS name,"
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
        exact_target_preservation(self.plan["installed_target_catalog"], self.catalog())
        require(self.append_slots() == self.plan["installed_append_slots"], "target physical append layout changed")
        if self.plan["preservation_sha256"]:
            self.approval()
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
        for obj in additions:
            if obj["kind"] == "column":
                obj["definition"]["position"] = slots[obj["name"].rsplit(".", 1)[0]] + 1
        return additions

    def pending_delta(self):
        return pending_delta(self.plan["contract"], len(self.plan["installed_history"]))

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
        config = self.stage / (purpose + ".compose.json")
        scratch_root = checked_path(self.stage, directory=True)
        scratch = Path(tempfile.mkdtemp(prefix=project + "-", dir=scratch_root))
        data_directory = scratch / "postgres"
        # PostgreSQL 18 creates/chowns its versioned PGDATA child, not the mount
        # root. Traversal here is safe inside the operator's 0700 scratch parent.
        data_directory.mkdir(mode=0o755)
        os.chmod(data_directory, 0o755)
        write_json(config, {"networks": {"default": {"internal": True}}, "services": {"postgres": {
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
        def isolated_sql(query):
            return run(database["compose"] + ["exec", "-T", "postgres", "psql", "-X", "-qAt",
                                              "-v", "ON_ERROR_STOP=1", "-U", "piping", "-d", database.get("name", "piping")],
                       data=query.encode()).strip()
        self.sql = isolated_sql
        try:
            yield
        finally:
            self.sql = production_sql

    def isolated_migrate(self, database, image):
        require(image in (self.image("backend"), self.plan.get("installed_backend_image")),
                "unverified migration executable")
        name = "piping-migrator-" + uuid.uuid4().hex
        label = "piping.release-stage=" + self.stage.name
        try:
            run(["docker", "run", "--name", name, "--label", label, "--pull", "never",
                 "--network", database["network"], "--cpus", "1", "--memory", "1g",
                 "--env-file", str(self.stage / "proof-backend.env"),
                 "-e", "PIPING_DATABASE__URL=" + database["url"],
                 "-e", "PIPING_RUN_MODE=backend",
                 "-e", "PIPING_START_MODE=migrate", "-e", "PIPING_START_EMBEDDED_GUACD=false", image], timeout=300)
        finally:
            ids = run(["docker", "ps", "-aq", "--filter", "name=^/" + name + "$",
                       "--filter", "label=" + label]).decode().splitlines()
            require(len(ids) <= 1, "ambiguous isolated migrator ownership")
            if ids:
                run(["docker", "rm", "-f", ids[0]])

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
            self.source_readiness(source_catalog, database)
            self.plan["source_roundtrip_checks"] = self.source_roundtrip_checks(database, source_catalog)
            if pending:
                self.isolated_migrate(database, self.image("backend"))
                with self.database_target(database):
                    require(self.catalog() == expected_catalog(source_catalog, self.plan["contract"], installed_version),
                            "published source has undeclared migration delta")
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
                                          replacements=self.pending_delta()["replacements"])
                self.guard_history(completed=True)
            self.plan["restored_candidate_catalog"] = candidate
        self.plan["installed_candidate_additions"] = self.target_additions(self.plan["installed_append_slots"])
        self.plan["installed_candidate_replacements"] = self.pending_delta()["replacements"]

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
        with self.isolated_database("restore") as database:
            with (self.stage / "offline-pre-upgrade.dump").open("rb") as source, tempfile.TemporaryFile() as output:
                result = subprocess.run(database["compose"] + ["exec", "-T", "postgres", "pg_restore",
                                        "--exit-on-error", "--no-owner", "--no-privileges", "-U", "piping", "-d", "piping"],
                                        stdin=source, stdout=output, stderr=output, timeout=300,
                                         env=environment(), check=False)
                if result.returncode:
                    output.seek(0)
                    write(self.stage / "restore-native.log", output.read(MAX_BYTES))
            require(result.returncode == 0, "isolated backup restore failed")
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
            self.isolated_migrate(database, self.image("backend"))
            with self.database_target(database):
                self.guard_database(completed=True)
                self.verify_conservation(baseline, restored=True)

    def fingerprint(self, columns=None):
        catalog = self.catalog()
        live_columns = json.loads(
            self.sql(
                "SELECT coalesce(json_object_agg(name,cols),'{}'::json) FROM ("
                "SELECT table_schema || '.' || table_name AS name,json_agg(column_name ORDER BY ordinal_position) AS cols "
                "FROM information_schema.columns WHERE table_schema NOT LIKE 'pg\\_%' ESCAPE '\\' "
                "AND table_schema <> 'information_schema' GROUP BY table_schema,table_name) c;"
            )
        )
        columns = live_columns if columns is None else columns
        require(set(columns) == set(live_columns), "database table coverage changed")
        statements = []
        for table, names in columns.items():
            require(set(names) <= set(live_columns[table]), "old database column missing")
            identifier = ".".join('"' + part.replace('"', '""') + '"' for part in table.split("."))
            fields = ",".join('"' + name.replace('"', '""') + '"' for name in names)
            query = (
                "SELECT count(*)::text || ':' || "
                "encode(sha256(convert_to(coalesce(string_agg(h,'' ORDER BY h),''),'UTF8')),'hex') FROM "
                "(SELECT encode(sha256(convert_to(to_jsonb(t)::text,'UTF8')),'hex') AS h FROM "
                "(SELECT " + fields + " FROM " + identifier + ") t) hashed"
            )
            statements.append((table, query))
        for obj in catalog:
            if obj["kind"] == "sequence":
                identifier = ".".join('"' + part.replace('"', '""') + '"' for part in obj["name"].split("."))
                statements.append(("sequence:" + obj["name"],
                                   "SELECT last_value::text || ':' || is_called::text FROM " + identifier))
        # One native server-side batch retains complete table coverage without a
        # subprocess per table. Only a transaction-local temporary receipt is written.
        def literal(value):
            return "'" + value.replace("'", "''") + "'"
        batch = (
            "BEGIN; SET LOCAL standard_conforming_strings=on; SET LOCAL TimeZone='UTC'; "
            "CREATE TEMP TABLE piping_row_receipt(name text PRIMARY KEY,fingerprint text) ON COMMIT DROP; "
            "DO $piping_receipt$ DECLARE digest text; BEGIN "
            + " ".join("EXECUTE " + literal(query) + " INTO digest; INSERT INTO piping_row_receipt VALUES ("
                       + literal(name) + ",digest);" for name, query in statements)
            + " END $piping_receipt$; SELECT coalesce(json_object_agg(name,fingerprint),'{}'::json) "
              "FROM piping_row_receipt; COMMIT;"
        )
        require("$piping_receipt$" not in "".join(name + query for name, query in statements),
                "unsafe database receipt identifier")
        rows = json.loads(self.sql(batch))
        return {"catalog": catalog, "columns": columns, "rows": rows, "history": self.history()}

    def verify_upgrade(self, baseline):
        self.guard_database(completed=True)
        self.verify_conservation(baseline)

    def verify_conservation(self, baseline, *, restored=False):
        actual = self.fingerprint(baseline["columns"])
        if restored:
            exact_target_preservation(self.plan["restored_candidate_catalog"], actual["catalog"])
        else:
            exact_target_preservation(self.plan["installed_target_catalog"], actual["catalog"],
                                      additions=self.plan["installed_candidate_additions"],
                                      replacements=self.plan["installed_candidate_replacements"])
            if self.plan["preservation_sha256"]:
                require(self.installed_check_bindings() == self.plan["installed_check_bindings"],
                        "migration changed original native CHECK bindings")
                if self.plan["check_capture_sha256"]:
                    self.guard_original_check_identity()
        old_rows = dict(baseline["rows"])
        new_rows = dict(actual["rows"])
        # The ledger is checked as a precise append above, never globally ignored.
        old_rows.pop("public._sqlx_migrations")
        new_rows.pop("public._sqlx_migrations")
        require(new_rows == old_rows, "migration changed existing rows or ciphertext")
        require(actual["history"][:len(baseline["history"])] == baseline["history"],
                "migration rewrote old ledger metadata")
        if len(baseline["history"]) < len(self.plan["migrations"]):
            for obj in self.pending_delta()["additions"]:
                if obj["kind"] == "column":
                    _, table, column = obj["name"].split(".")
                    require(self.sql('SELECT NOT EXISTS (SELECT 1 FROM public."' + table + '" WHERE "'
                                      + column + '" IS DISTINCT FROM ' + obj["definition"]["default"] + ');') == b"t",
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

    def guard_writer_ids(self):
        for service, expected in self.plan["writer_ids"].items():
            ids = run(self.compose() + ["ps", "-aq", service]).decode().splitlines()
            require(ids == [expected], "installed writer identity changed since staging")

    def backup(self):
        self.load("staged")
        self.guard_config()
        self.guard_writer_ids()
        self.inspect_images()
        self.guard_database()
        self.guard_target_catalog()
        require(self.service_image("postgres") == self.plan["postgres_image"], "installed PostgreSQL image changed")
        # Read-only tool/capacity preflight precedes writer stop. Image exports are
        # carried to the leader too; no registry availability is assumed there.
        run(["openssl", "version"])
        size = int(self.sql("SELECT pg_database_size(current_database());"))
        require(shutil.disk_usage(self.stage).free >= max(size * 3, 1024 * 1024 * 1024),
                "insufficient disk capacity for backup and native image archives")
        postgres_archive = save_image(self.plan["postgres_image"], self.stage, "installed-postgres", None,
                                      self.plan["platform"])
        installed_archive = None
        if len(self.plan["installed_history"]) < len(self.plan["migrations"]):
            installed_archive = save_image(self.plan["installed_backend_image"], self.stage, "installed-backend", None,
                                           self.plan["platform"])
        require(shutil.disk_usage(self.stage).free >= max(size * 2, 1024 * 1024 * 1024),
                "insufficient disk capacity after image export")
        writers = [service for service in WRITERS if service in self.running()]
        require(
            set(writers) >= {"backend", "ansible-runner"},
            "required writers are not running",
        )
        self.advance("backup-starting")
        write_json(self.stage / "writers.json", writers)
        self.advance("writers-stopping")
        run(self.compose() + ["stop", "--timeout", "60", *writers])
        self.advance("writers-stopped")
        self.stopped()
        self.guard_database()
        self.guard_config()
        self.guard_target_catalog()
        fingerprint = self.fingerprint()
        self.advance("dumping")
        dump = self.stage / "offline-pre-upgrade.dump"
        checked_path(dump, missing=True)
        with dump.open("xb") as stream:
            os.chmod(dump, 0o600)
            run(
                self.postgres_command(
                    "sh",
                    "-c",
                    'exec pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" --format=custom',
                ),
                output=stream,
                timeout=300,
            )
            stream.flush()
            os.fsync(stream.fileno())
        require(dump.stat().st_size > 0, "fresh dump is empty")
        # Stream the archive into native pg_restore without materializing it in memory.
        with dump.open("rb") as stream, tempfile.TemporaryFile() as captured:
            result = subprocess.run(
                self.postgres_command("pg_restore", "--list"),
                stdin=stream,
                stdout=captured,
                stderr=captured,
                timeout=120,
                env=environment(),
                check=False,
            )
        require(result.returncode == 0, "fresh archive validation failed")
        require(self.fingerprint() == fingerprint, "database changed during fresh backup")
        write_json(self.stage / "fingerprint.json", fingerprint)
        self.guard_database()
        self.stopped()
        self.guard_config()
        request = {"schema": "piping.compose-backup/v1", "nonce": uuid.uuid4().hex,
                   "target": socket.gethostname(), "root": str(self.root), "stage": str(self.stage),
                   "project_name": self.args.project_name, "helper_sha256": helper_identity(),
                   "commit": self.plan["commit"], "pipeline_id": self.plan["pipeline_id"],
                   "package_sha256": self.plan["package_sha256"], "plan_sha256": file_sha(self.stage / "plan.json"),
                   "preservation_sha256": self.plan["preservation_sha256"],
                   "postgres_archive": postgres_archive, "installed_backend_archive": installed_archive,
                   "database_bytes": size,
                   "files": {name: {"sha256": file_sha(self.stage / name), "size": (self.stage / name).stat().st_size}
                             for name in ("offline-pre-upgrade.dump", "fingerprint.json", "proof-backend.env",
                                          "installed-target-schema.dump",
                                           *(["preservation.json"] if self.plan["preservation_sha256"] else []),
                                           *(["check-capture.json"] if self.plan["check_capture_sha256"] else []))}}
        write_json(self.stage / "backup-request.json", request)
        self.seal(
            "backup-seal.json",
            ["offline-pre-upgrade.dump", "fingerprint.json", "writers.json", "backup-request.json",
             postgres_archive["archive"], *([installed_archive["archive"]] if installed_archive else [])],
        )
        self.advance("backup-verified")

    def consume_receipt(self):
        require(not os.path.lexists(self.stage / "consumed-receipt.json"), "restore receipt already consumed")
        receipt_path, signature = self.args.receipt, self.args.receipt_signature
        authenticate(receipt_path, signature, self.stage / "verification-key.pem")
        receipt = read_json(receipt_path)
        request = read_json(self.stage / "backup-request.json")
        require(receipt["schema"] == "piping.compose-restore-receipt/v1"
                and receipt["result"] == "native-restore-migration-conservation-passed"
                and receipt["request_sha256"] == file_sha(self.stage / "backup-request.json")
                and receipt["request"] == request
                and request["helper_sha256"] == helper_identity()
                and request["package_sha256"] == self.plan["package_sha256"]
                and request["preservation_sha256"] == self.plan["preservation_sha256"]
                and request["plan_sha256"] == file_sha(self.stage / "plan.json")
                and request["target"] == self.plan["target"] == socket.gethostname()
                and request["root"] == str(self.root) and request["stage"] == str(self.stage)
                and request["project_name"] == self.args.project_name
                and request["commit"] == self.plan["commit"] and request["pipeline_id"] == self.plan["pipeline_id"],
                "restore receipt binding mismatch")
        for name, description in request["files"].items():
            require(Path(name).name == name and file_sha(self.stage / name) == description["sha256"]
                    and (self.stage / name).stat().st_size == description["size"], "fresh backup changed after leader proof")
        write_json(self.stage / "consumed-receipt.json", {"receipt_sha256": file_sha(receipt_path),
                   "request_sha256": receipt["request_sha256"], "nonce": request["nonce"]})

    def apply(self):
        self.load("backup-verified")
        self.verify_seal("backup-seal.json")
        self.guard_config()
        self.guard_writer_ids()
        self.inspect_images()
        self.stopped()
        self.guard_database()
        require(
            self.fingerprint() == read_json(self.stage / "fingerprint.json"),
            "database changed after backup",
        )
        self.guard_target_catalog()
        baseline = read_json(self.stage / "fingerprint.json")
        self.consume_receipt()
        self.advance("restore-starting")
        self.stopped()
        self.guard_config()
        self.guard_database()
        require(self.fingerprint() == baseline, "production changed during isolated restore proof")
        self.advance("restore-verified")
        name = self.args.project_name + "-" + self.stage.name + "-migrate"
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
            "migration container already exists",
        )
        self.advance("apply-starting")
        temporary = self.root / (self.plan["image_file"] + "." + self.stage.name)
        self.advance("images-installing")
        write(temporary, read(self.stage / "images.next"))
        self.guard_config()
        os.replace(temporary, self.root / self.plan["image_file"])
        sync_directory(self.root)
        self.guard_config(installed=True)
        self.stopped()
        self.guard_database()
        require(
            self.fingerprint() == read_json(self.stage / "fingerprint.json"),
            "database changed before migration",
        )
        self.advance("migrating")
        try:
            run(
                self.compose()
                + [
                    "-f",
                    str(self.stage / "migrate.compose.yml"),
                    "run",
                    "--name",
                    name,
                    "--no-deps",
                    "--pull",
                    "never",
                    "-l",
                    "piping.release-stage=" + self.stage.name,
                    "-e",
                    "PIPING_START_MODE=migrate",
                    "-e",
                    "PIPING_RUN_MODE=backend",
                    "-e",
                    "PIPING_START_EMBEDDED_GUACD=false",
                    "backend",
                ],
                timeout=300,
            )
        finally:
            # Only a container carrying this operation's ownership label can be removed.
            ids = (
                run(
                    [
                        "docker",
                        "ps",
                        "-aq",
                        "--filter",
                        "name=^/" + name + "$",
                        "--filter",
                        "label=piping.release-stage=" + self.stage.name,
                    ]
                )
                .decode()
                .splitlines()
            )
            require(len(ids) <= 1, "ambiguous owned migration container")
            if ids:
                run(["docker", "stop", "--time", "30", ids[0]], timeout=60)
                run(["docker", "rm", ids[0]])
            self.stopped()
        self.guard_database(completed=True)
        self.guard_config(installed=True)
        self.verify_upgrade(baseline)
        self.advance("migration-verified")
        writers = read_json(self.stage / "writers.json")
        self.advance("starting")
        try:
            run(
                self.compose()
                + [
                    "up",
                    "-d",
                    "--no-deps",
                    "--pull",
                    "never",
                    "--no-build",
                    *writers,
                    "frontend",
                ],
                timeout=300,
            )
            require(set(writers + ["frontend"]) <= self.running(), "partial startup")
            self.guard_config(installed=True)
            self.advance("started-awaiting-health")
        except BaseException:
            # Timeout, signal or partial startup must leave all former writers stopped.
            run(self.compose() + ["stop", "--timeout", "60", *writers])
            self.stopped()
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
    for command in (stage_parser, backup_parser, apply_parser, health_parser):
        command.add_argument("--root", required=True, type=Path)
        command.add_argument("--stage", required=True, type=Path)
        command.add_argument("--project-name", required=True)
    for command in (prepare_parser, verify_parser, stage_parser):
        command.add_argument("--package", required=True, type=Path)
    for command in (prepare_parser, verify_parser):
        command.add_argument("--signing-key", required=True, type=Path)
    for command in (verify_parser, stage_parser):
        command.add_argument("--verification-key", required=True, type=Path)
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


def record_failure(deployment, reason):
    # Deployment boundary reasons contain no command output or argument values.
    # A diagnostic failure must never mask the original fail-closed outcome.
    with contextlib.suppress(OSError, ValueError, KeyError, TypeError):
        phase = deployment.phase()
        write_json(deployment.stage / ("failure-" + uuid.uuid4().hex + ".json"),
                   {"phase": phase, "reason": reason})


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
                "apply": deployment.apply,
                "verify-health": deployment.verify_runtime_health,
            }[args.action]()
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
            record_failure(deployment, str(error) if isinstance(error, DeployError) else "invalid native boundary")
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
