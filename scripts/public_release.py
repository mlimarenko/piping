"""Public release cohort, immutable publication and verified installer transport.

The GitHub Release manifest is the readiness boundary, not an image tag.
Only the installation engine in install.sh changes installation state.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath

REPOSITORY = "mlimarenko/piping"
PLATFORM = "linux/amd64"
MANIFEST = "piping-public-release.json"
IMAGES = {
    "backend": "docker.io/pipingspace/piping-backend",
    "frontend": "docker.io/pipingspace/piping-frontend",
    "ansible-runner": "docker.io/pipingspace/piping-ansible-runner",
}
ENV_KEYS = {"backend": "PIPING_BACKEND_IMAGE", "frontend": "PIPING_FRONTEND_IMAGE",
            "ansible-runner": "PIPING_ANSIBLE_RUNNER_IMAGE"}
MAX_ARCHIVE = 64 * 1024 * 1024
MAX_EXPANDED = 128 * 1024 * 1024
DOWNLOAD_REDIRECT_HOSTS = {
    "github.com": {"release-assets.githubusercontent.com", "objects.githubusercontent.com",
                   "github-releases.githubusercontent.com"},
    "registry-1.docker.io": {"production.cloudflare.docker.com", "production.cloudfront.docker.com"},
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def matches(pattern, value):
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None


def version(value):
    require(matches(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", value)
            and len(value) <= 64, "expected stable X.Y.Z version")
    return value


def exact(value, keys):
    require(isinstance(value, dict) and set(value) == set(keys), "unexpected manifest fields")


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON key")
        result[key] = value
    return result


def decode(raw):
    require(len(raw) <= 1024 * 1024, "JSON exceeds limit")
    return json.loads(raw, object_pairs_hook=unique)


def canonical(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def source_descriptor(value, selected):
    exact(value, {"schema", "version", "commit", "pipeline_id"})
    require(value["schema"] == "piping.public-source/v1" and value["version"] == version(selected),
            "source snapshot version mismatch")
    require(matches(r"[0-9a-f]{40}", value["commit"]), "invalid canonical source commit")
    require(matches(r"[1-9][0-9]{0,19}", value["pipeline_id"]), "invalid source pipeline")
    return value


def archive_name(selected):
    return f"piping-{version(selected)}-linux-amd64.tar.gz"


def validate_manifest(value, selected, source, public_commit):
    source_descriptor(source, selected)
    exact(value, {"schema", "version", "public_commit", "source_commit", "pipeline_id",
                  "platform", "images", "installation"})
    require(matches(r"[0-9a-f]{40}", public_commit), "invalid public commit")
    require(value["schema"] == "piping.public-release/v1" and value["version"] == selected
            and value["public_commit"] == public_commit and value["source_commit"] == source["commit"]
            and value["pipeline_id"] == source["pipeline_id"] and value["platform"] == PLATFORM,
            "release/snapshot association mismatch")
    exact(value["images"], IMAGES)
    for role, repository in IMAGES.items():
        image = value["images"][role]
        exact(image, {"repository", "digest"})
        require(image["repository"] == repository and matches(r"sha256:[0-9a-f]{64}", image["digest"]),
                "invalid public image repository or digest")
    item = value["installation"]
    exact(item, {"name", "size_bytes", "sha256"})
    require(item["name"] == archive_name(selected) and type(item["size_bytes"]) is int
            and 0 < item["size_bytes"] <= MAX_ARCHIVE and matches(r"[0-9a-f]{64}", item["sha256"]),
            "invalid installation asset")
    return value


def images_env(images):
    return "".join(f"{ENV_KEYS[role]}={IMAGES[role]}@{images[role]['digest']}\n" for role in IMAGES).encode()


def run(arguments, cwd=None):
    return subprocess.run(arguments, cwd=cwd, check=True, capture_output=True, timeout=60).stdout


def payload_paths(root):
    # The installer is the only owner of its payload list. Source it natively,
    # with no main invocation; do not parse shell or maintain another file list.
    raw = run(["bash", "-c", 'source "$1/install.sh"; printf "%s\\0" "${STACK_SOURCE_PATHS[@]}"',
               "public-release", str(root)])
    names = raw.decode().rstrip("\0").split("\0")
    require(0 < len(names) <= 128 and len(set(names)) == len(names), "invalid installer payload list")
    for name in names:
        safe_name(name)
    require("install.sh" in names and "env/images.env" in names, "incomplete installer payload")
    return names


def safe_name(name):
    path = PurePosixPath(name)
    require(isinstance(name, str) and name and not path.is_absolute()
            and str(path) == name and all(part not in {".", ".."} and not part.startswith(".")
                                         for part in path.parts)
            and "\\" not in name and not any(ord(c) < 32 for c in name), "unsafe archive path")


def regular_file(root, name):
    safe_name(name)
    path = root
    for component in PurePosixPath(name).parts:
        path = path / component
        require(not path.is_symlink(), "symlink in installation payload")
    require(path.is_file() and path.stat().st_size <= 32 * 1024 * 1024, "missing or oversized payload file")
    return path


def build_archive(root, source, images):
    names = payload_paths(root)
    require("env/public-source.json" not in names, "source descriptor duplicated in installer payload")
    names.append("env/public-source.json")
    output = io.BytesIO()
    with (gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed,
          tarfile.open(fileobj=compressed, mode="w") as archive):
        total = 0
        for name in sorted(names):
            if name == "env/images.env":
                data, mode = images_env(images), 0o644
            else:
                path = regular_file(root, name)
                data, mode = path.read_bytes(), path.stat().st_mode & 0o777
                require(not path.stat().st_mode & 0o7022, "unsafe installation file mode")
            if name == "env/public-source.json":
                require(decode(data) == source, "source descriptor changed during packaging")
            total += len(data)
            require(total <= MAX_EXPANDED, "installation payload too large")
            item = tarfile.TarInfo(name)
            item.size, item.mode = len(data), mode
            archive.addfile(item, io.BytesIO(data))
    raw = output.getvalue()
    require(len(raw) <= MAX_ARCHIVE, "installation archive too large")
    return raw


def extract_archive(raw, manifest, destination):
    item = manifest["installation"]
    require(len(raw) == item["size_bytes"] and hashlib.sha256(raw).hexdigest() == item["sha256"],
            "installation archive size/checksum mismatch")
    # Bound decompression before tarfile parses extension headers, whose sizes
    # otherwise precede (and can bypass) the regular-member budget below.
    with gzip.GzipFile(fileobj=io.BytesIO(raw), mode="rb") as compressed:
        unpacked = compressed.read(MAX_EXPANDED + 1024 * 1024 + 1)
    require(len(unpacked) <= MAX_EXPANDED + 1024 * 1024, "expanded tar stream exceeds limit")
    # Validate every header before creating any files. Never delegate extraction
    # to tar's path/link handling, and never restore special ownership or bits.
    with tarfile.open(fileobj=io.BytesIO(unpacked), mode="r:") as archive:
        members, seen, total = [], set(), 0
        for member in archive:
            safe_name(member.name)
            require(member.type in {tarfile.REGTYPE, tarfile.AREGTYPE} and not member.pax_headers
                    and member.name not in seen and len(seen) < 128
                    and 0 <= member.size <= 32 * 1024 * 1024 and not member.mode & 0o7022,
                    "unsafe or duplicate installation member")
            seen.add(member.name)
            total += member.size
            require(total <= MAX_EXPANDED, "expanded installation exceeds limit")
            members.append(member)
        require({"install.sh", "env/images.env", "env/public-source.json"} <= seen,
                "incomplete installation archive")
        for member in members:
            target = destination / member.name
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.extractfile(member) as stream, target.open("xb") as output:
                output.write(stream.read())
            target.chmod(member.mode & 0o777)
    source = source_descriptor(decode((destination / "env/public-source.json").read_bytes()), manifest["version"])
    validate_manifest(manifest, manifest["version"], source, manifest["public_commit"])
    require((destination / "env/images.env").read_bytes() == images_env(manifest["images"]),
            "archive image cohort mismatch")
    require(seen == set(payload_paths(destination)) | {"env/public-source.json"},
            "archive differs from installer payload contract")


def https_origin(url):
    parsed = urllib.parse.urlsplit(url)
    require(parsed.scheme == "https" and parsed.hostname and not parsed.username
            and not parsed.password and not parsed.fragment, "HTTPS URL without credentials required")
    return parsed.hostname, parsed.port or 443


class SecureRedirect(urllib.request.HTTPRedirectHandler):
    def __init__(self, original):
        self.origin = https_origin(original)
        self.download_hosts = DOWNLOAD_REDIRECT_HOSTS.get(self.origin[0], set())

    def redirect_request(self, request, response, code, message, headers, newurl):
        try:
            target = https_origin(newurl)
            require(request.get_method() in {"GET", "HEAD"}, "mutating requests may not redirect")
            require(target == self.origin or (target[0] in self.download_hosts and target[1] == 443),
                    "unapproved redirect origin")
        except ValueError:
            response.close()
            raise
        redirected = super().redirect_request(request, response, code, message, headers, newurl)
        if target != https_origin(request.full_url):
            # urllib normally copies Authorization across origins. Downloads
            # need only content negotiation, never API/registry tokens or cookies.
            redirected.headers = {key: value for key, value in redirected.headers.items()
                                  if key.lower() in {"accept", "user-agent"}}
            redirected.unredirected_hdrs = {}
        return redirected


class Network:
    def __init__(self, *, tls_context=None):
        self.tls_context = tls_context

    def request(self, url, *, limit=1024 * 1024, headers=None, data=None, method=None):
        https_origin(url)
        request = urllib.request.Request(url, headers=headers or {}, data=data, method=method)
        opener = urllib.request.build_opener(SecureRedirect(url), urllib.request.HTTPSHandler(context=self.tls_context))
        with opener.open(request, timeout=30) as response:
            body = response.read(limit + 1)
            require(len(body) <= limit, "network response exceeds limit")
            return body

    def github(self, path, *, data=None, method=None):
        headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}
        if os.environ.get("GH_TOKEN"):
            headers["Authorization"] = "Bearer " + os.environ["GH_TOKEN"]
        endpoint = f"https://api.github.com/repos/{REPOSITORY}" + (f"/{path}" if path else "")
        return decode(self.request(endpoint,
                                   headers=headers, data=data, method=method))

    def asset(self, selected, name, limit=MAX_ARCHIVE):
        require(name in {MANIFEST, archive_name(selected)}, "unapproved release asset")
        return self.request(f"https://github.com/{REPOSITORY}/releases/download/{version(selected)}/{name}", limit=limit)

    def snapshot(self, selected):
        return source_descriptor(decode(self.request(
            f"https://raw.githubusercontent.com/{REPOSITORY}/{version(selected)}/env/public-source.json")), selected)

    def public_commit(self, selected):
        value = self.github(f"commits/{version(selected)}")["sha"]
        require(matches(r"[0-9a-f]{40}", value), "invalid public tag target")
        return value


class ChannelError(ValueError):
    """An actionable channel failure whose message contains no remote content."""


def channel_metadata(value, repository, *, writable):
    # Docker Hub returns 0/initialized for a newly created empty channel and
    # 1/Active for an active channel. Status is not a write-permission signal.
    require(isinstance(value, dict) and value.get("namespace") == "pipingspace"
            and value.get("name") == repository and value.get("is_private") is False
            and value.get("repository_type") == "image"
            and type(value.get("status")) is int and value["status"] in {0, 1}
            and isinstance(value.get("status_description"), str)
            and value["status_description"].lower() == {0: "initialized", 1: "active"}[value["status"]]
            and value.get("is_automated") is False,
            "DockerHub channel is private, foreign or inconsistent")
    if writable:
        require(isinstance(value.get("permissions"), dict) and value["permissions"].get("write") is True,
                "DockerHub channel is not writable")


def initialize_channels(root, selected, network):
    # Validate the selected published Release before any authorized channel write.
    context(root, selected, network)
    username, secret = os.environ.get("DOCKERHUB_USERNAME"), os.environ.get("DOCKERHUB_TOKEN")
    if not username or not secret:
        raise ChannelError("Set DOCKERHUB_USERNAME and DOCKERHUB_TOKEN with repository creation/write rights in pipingspace")

    def hub(path, *, headers=None, data=None, missing=False):
        try:
            return decode(network.request("https://hub.docker.com/v2/" + path, headers=headers,
                                          data=data, method="POST" if data is not None else "GET"))
        except urllib.error.HTTPError as error:
            code = error.code
            error.close()
            if missing and code == 404:
                return None  # Authenticated lookup only; create still refuses any existing name.
            raise ChannelError(f"DockerHub channel API refused HTTP {code}; check DOCKERHUB_USERNAME/DOCKERHUB_TOKEN "
                               "and pipingspace repository creation/write rights; existing visibility is never changed") from None

    authentication = hub("auth/token", headers={"Content-Type": "application/json"},
                         data=canonical({"identifier": username, "secret": secret}))
    require(isinstance(authentication, dict), "invalid DockerHub authentication response")
    access = authentication.get("access_token")
    require(matches(r"[A-Za-z0-9._~-]{1,16384}", access), "invalid DockerHub access token response")
    authenticated = {"Authorization": "Bearer " + access, "Content-Type": "application/json"}
    missing = []
    try:
        # Inspect all existing channels before creating any. No anonymous 404,
        # visibility update, adoption or registry 401 can authorize creation.
        for image in IMAGES.values():
            repository = image.removeprefix("docker.io/pipingspace/")
            metadata = hub("namespaces/pipingspace/repositories/" + repository,
                           headers=authenticated, missing=True)
            if metadata is None:
                missing.append(repository)
            else:
                channel_metadata(metadata, repository, writable=True)
        for repository in missing:
            created = hub("namespaces/pipingspace/repositories", headers=authenticated,
                          data=canonical({"name": repository, "namespace": "pipingspace",
                                          "registry": "docker.io", "is_private": False}))
            channel_metadata(created, repository, writable=True)
        # Read back authenticated metadata and independently prove public access.
        for image in IMAGES.values():
            repository = image.removeprefix("docker.io/pipingspace/")
            path = "namespaces/pipingspace/repositories/" + repository
            channel_metadata(hub(path, headers=authenticated), repository, writable=True)
            channel_metadata(hub(path), repository, writable=False)
    except ValueError as error:
        if isinstance(error, ChannelError):
            raise
        raise ChannelError("DockerHub channel must be the exact public, initialized or active, writable pipingspace image repository; "
                           "private/foreign channels are refused, not exposed or changed") from None
    print("Public DockerHub channels established for the exact three release repositories")


def labels(source, public_commit):
    return {"org.opencontainers.image.revision": public_commit,
            "org.opencontainers.image.version": source["version"],
            "org.opencontainers.image.source": f"https://github.com/{REPOSITORY}",
            "space.piping.source.commit": source["commit"],
            "space.piping.source.pipeline": source["pipeline_id"]}


def registry_image(network, role, source, public_commit):
    repository = IMAGES[role].removeprefix("docker.io/")
    token = decode(network.request("https://auth.docker.io/token?service=registry.docker.io&scope=repository:"
                                   + repository + ":pull"))["token"]
    headers = {"Authorization": "Bearer " + token,
               "Accept": ("application/vnd.oci.image.index.v1+json,"
                          "application/vnd.oci.image.manifest.v1+json,"
                          "application/vnd.docker.distribution.manifest.list.v2+json,"
                          "application/vnd.docker.distribution.manifest.v2+json")}
    base = "https://registry-1.docker.io/v2/" + repository
    try:
        raw = network.request(base + "/manifests/" + source["version"], headers=headers)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            error.close()
            return None  # Only a registry-authoritative missing tag permits a build.
        raise
    digest = "sha256:" + hashlib.sha256(raw).hexdigest()
    manifest = decode(raw)
    if "manifests" in manifest:
        candidates = [entry for entry in manifest["manifests"]
                      if entry.get("platform", {}).get("os") == "linux"
                      and entry.get("platform", {}).get("architecture") == "amd64"]
        require(len(candidates) == 1, "ambiguous registry platform")
        # BuildKit attestations have unknown/unknown platform; other runnable
        # platforms would silently widen this channel's installation contract.
        require(all(entry in candidates or entry.get("platform") == {"architecture": "unknown", "os": "unknown"}
                    for entry in manifest["manifests"]), "foreign registry platform")
        child = candidates[0]["digest"]
        require(matches(r"sha256:[0-9a-f]{64}", child), "invalid registry manifest digest")
        raw = network.request(base + "/manifests/" + child, headers=headers)
        require("sha256:" + hashlib.sha256(raw).hexdigest() == child, "registry manifest digest mismatch")
        manifest = decode(raw)
    config_digest = manifest["config"]["digest"]
    require(matches(r"sha256:[0-9a-f]{64}", config_digest), "invalid image config digest")
    raw = network.request(base + "/blobs/" + config_digest, headers=headers)
    require("sha256:" + hashlib.sha256(raw).hexdigest() == config_digest, "registry config digest mismatch")
    config = decode(raw)
    require(config.get("os") == "linux" and config.get("architecture") == "amd64", "image platform mismatch")
    actual = config.get("config", {}).get("Labels") or {}
    require(all(actual.get(key) == value for key, value in labels(source, public_commit).items()),
            "immutable public tag has foreign revision/version/provenance")
    return {"repository": IMAGES[role], "digest": digest}


def installed_arguments(arguments):
    # Let install.sh validate every option; inspect only its existing directory
    # selector so a bare bootstrap rerun enters the installed engine, not a new
    # version's source tree. Explicit image flags still use its guarded updater.
    directory = Path.home() / "piping"
    for index, arg in enumerate(arguments):
        if arg == "--install-dir" and index + 1 < len(arguments):
            directory = Path(arguments[index + 1]).expanduser().absolute()
        elif arg == "--install-dir":
            raise ValueError("missing installation directory")
    return directory


def installed_engine(destination):
    require(destination != Path.home() and destination != Path(destination.anchor),
            "choose a stack-specific directory")
    require(not any(path.is_symlink() for path in (destination, *destination.parents)),
            "installation path must not contain symlinks")
    if not destination.exists():
        return None
    require(destination.is_dir(), "installation destination must be a directory")
    if not any(destination.iterdir()):
        return None
    require((destination / ".env").is_file() and not (destination / ".env").is_symlink(),
            "existing installation settings are missing; restore the original .env")
    regular_file(destination, "install.sh")
    for name in payload_paths(destination):
        regular_file(destination, name)
    return destination / "install.sh"


def install(selected, arguments, network=None):
    version(selected)
    require("--source-base-url" not in arguments,
            "public installation uses only the selected release transport")
    destination = installed_arguments(arguments)
    engine = installed_engine(destination)
    if engine is not None:
        # Running a newer public command is not an image or installer upgrade.
        # The installed owner retains settings, TLS, locks and update policy.
        subprocess.run(["bash", str(engine), *arguments], check=True)
        return
    require(not any(option in arguments for option in ("--backend-image", "--frontend-image", "--ansible-runner-image")),
            "fresh public installation uses the selected release image cohort")
    network = network or Network()
    public_commit = network.public_commit(selected)
    source = network.snapshot(selected)
    manifest = validate_manifest(decode(network.asset(selected, MANIFEST, 1024 * 1024)),
                                 selected, source, public_commit)
    raw = network.asset(selected, manifest["installation"]["name"], manifest["installation"]["size_bytes"])
    with tempfile.TemporaryDirectory(prefix="piping-install-") as temporary:
        root = Path(temporary)
        extract_archive(raw, manifest, root)
        engine = root / "install.sh"
        # Recheck after download, before executing a fresh package. The root
        # engine takes the actual deployment lock before any settings mutation.
        require(installed_engine(destination) is None, "installation destination changed during acquisition")
        # No shell interpolation of user options; the existing engine owns all
        # settings, image-update guards, locks, backups and data preservation.
        subprocess.run(["bash", str(engine), *arguments], check=True)


def context(root, selected, network):
    source = source_descriptor(decode((root / "env/public-source.json").read_bytes()), selected)
    public_commit = run(["git", "rev-parse", "HEAD"], cwd=root).decode().strip()
    require(public_commit == network.public_commit(selected), "checkout is not the selected public tag")
    require(source == network.snapshot(selected), "checkout descriptor differs from selected snapshot")
    release = network.github(f"releases/tags/{version(selected)}")
    require(release["tag_name"] == selected and not release["draft"] and not release["prerelease"]
            and release.get("published_at"), "only published stable GitHub Releases may build")
    require(len(release["assets"]) < 100, "release asset listing exceeds limit")
    return source, public_commit, release


def existing_asset(network, release, selected, name):
    assets = [asset for asset in release["assets"] if asset["name"] == name]
    require(len(assets) <= 1, "duplicate release asset")
    return network.asset(selected, name) if assets else None


def output_values(values):
    target = os.environ.get("GITHUB_OUTPUT")
    text = "".join(f"{key}={value}\n" for key, value in values.items())
    if target:
        with open(target, "a") as stream:
            stream.write(text)
    else:
        print(text, end="")


def prepare(root, selected, network):
    source, public_commit, release = context(root, selected, network)
    images = {role: registry_image(network, role, source, public_commit) for role in IMAGES}
    ready = existing_asset(network, release, selected, MANIFEST)
    if ready is not None:
        manifest = validate_manifest(decode(ready), selected, source, public_commit)
        require(manifest["images"] == images, "published cohort differs from immutable tags")
        raw = network.asset(selected, archive_name(selected))
        require(raw == build_archive(root, source, images), "published archive differs from selected source")
        require(len(raw) == manifest["installation"]["size_bytes"]
                and hashlib.sha256(raw).hexdigest() == manifest["installation"]["sha256"], "published archive conflict")
    values = {"complete": str(ready is not None).lower(), "version": selected, "public_commit": public_commit,
              "source_commit": source["commit"], "pipeline_id": source["pipeline_id"]}
    values.update({"build_" + role.replace("-", "_"): str(image is None).lower() for role, image in images.items()})
    output_values(values)


def upload_asset(network, release, selected, name, raw):
    existing = existing_asset(network, release, selected, name)
    if existing is not None:
        require(existing == raw, "conflicting release asset; refusing overwrite")
        return
    # Use the approved repository and numeric release ID, never upload_url from
    # untrusted metadata. GitHub's create API refuses duplicate asset names.
    require(type(release["id"]) is int and release["id"] > 0, "invalid release ID")
    token = os.environ.get("GH_TOKEN")
    require(token, "GH_TOKEN required for asset publication")
    network.request(f"https://uploads.github.com/repos/{REPOSITORY}/releases/{release['id']}/assets?name={name}",
                    headers={"Authorization": "Bearer " + token, "Content-Type": "application/octet-stream"},
                    data=raw, method="POST")


def finalize(root, selected, network, expected):
    source, public_commit, release = context(root, selected, network)
    images = {role: registry_image(network, role, source, public_commit) for role in IMAGES}
    require(all(images.values()), "incomplete public image cohort")
    for role, digest in expected.items():
        if digest:
            require(images[role]["digest"] == digest, "build output differs from public immutable tag")
    raw = build_archive(root, source, images)
    manifest = {"schema": "piping.public-release/v1", "version": selected, "public_commit": public_commit,
                "source_commit": source["commit"], "pipeline_id": source["pipeline_id"], "platform": PLATFORM,
                "images": images, "installation": {"name": archive_name(selected), "size_bytes": len(raw),
                                                     "sha256": hashlib.sha256(raw).hexdigest()}}
    validate_manifest(manifest, selected, source, public_commit)
    # Check an existing readiness marker BEFORE uploading anything.
    ready = existing_asset(network, release, selected, MANIFEST)
    require(ready is None or decode(ready) == manifest, "conflicting published cohort")
    upload_asset(network, release, selected, archive_name(selected), raw)
    upload_asset(network, release, selected, MANIFEST, canonical(manifest))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["initialize", "prepare", "finalize", "install"])
    parser.add_argument("--version", required=True)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    for role in IMAGES:
        parser.add_argument("--" + role + "-digest", default="")
    options, arguments = parser.parse_known_args()
    if arguments[:1] == ["--"]:
        arguments = arguments[1:]
    version(options.version)
    if options.command == "install":
        install(options.version, arguments)
    else:
        require(not arguments, "unexpected workflow arguments")
        require(os.environ.get("GITHUB_REPOSITORY") == REPOSITORY, "unapproved GitHub repository")
        if options.command == "initialize":
            initialize_channels(options.root.resolve(), options.version, Network())
        elif options.command == "prepare":
            prepare(options.root.resolve(), options.version, Network())
        else:
            expected = {role: getattr(options, role.replace("-", "_") + "_digest") for role in IMAGES}
            finalize(options.root.resolve(), options.version, Network(), expected)


if __name__ == "__main__":
    try:
        main()
    except ChannelError as error:
        print(f"piping public release: {error}", file=sys.stderr)
        sys.exit(1)
    except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError, tarfile.TarError) as error:
        # Do not render HTTP request objects or subprocess output: either may
        # contain authentication material from a remote failure.
        print(f"piping public release: {type(error).__name__}: operation refused", file=sys.stderr)
        sys.exit(1)
