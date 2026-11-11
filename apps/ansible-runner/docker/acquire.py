#!/usr/bin/env python3
"""Git acquisition inside a disposable sandbox, never the supervisor."""
import os
import json
import hashlib
import stat
import pathlib
import re
import subprocess
import sys
import tempfile
import threading
import urllib.parse


def remote_transport(remote: str) -> str:
    if any(ord(c) < 33 or ord(c) == 127 for c in remote):
        raise ValueError("invalid remote")
    if re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*@(?:[A-Za-z0-9][A-Za-z0-9.-]*|\[[0-9A-Fa-f:]+\]):[^\s]+", remote):
        return "ssh"
    url = urllib.parse.urlsplit(remote)
    if (url.scheme not in ("https", "ssh") or not url.hostname
            or url.password is not None or url.query or url.fragment
            or not re.fullmatch(r"[A-Za-z0-9:.\-]+", url.hostname)
            or not url.path.startswith("/") or url.path == "/"):
        raise ValueError("unsupported remote")
    _ = url.port
    if url.scheme == "https" and url.username is not None:
        raise ValueError("HTTPS userinfo is forbidden")
    if url.scheme == "ssh" and url.username is not None and not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*", url.username):
        raise ValueError("invalid SSH user")
    return url.scheme


def credential_response(controller: pathlib.Path, operation: str, data: bytes) -> bytes:
    if operation not in ("get", "store", "erase") or len(data) > 65536 or b"\0" in data:
        raise ValueError("unsupported credential operation")
    # This helper never stores credentials or emits them for notification calls.
    if operation != "get":
        return b""
    fields = {}
    for line in data.splitlines():
        if not line:
            break
        key, separator, value = line.partition(b"=")
        if not separator or not key:
            raise ValueError("invalid credential request")
        # Git credential protocol array attributes can legitimately repeat.
        # They carry announcements, not the scalar destination scope.
        if key.endswith(b"[]"):
            continue
        if key in fields:
            raise ValueError("invalid credential request")
        fields[key] = value
    remote = (controller / "remote").read_text(encoding="utf-8")
    if remote_transport(remote) != "https":
        raise ValueError("invalid credential scope")
    url = urllib.parse.urlsplit(remote)
    expected = {b"protocol": b"https", b"host": url.netloc.encode(),
                b"path": urllib.parse.unquote_to_bytes(url.path[1:])}
    if any(fields.get(key) != value for key, value in expected.items()):
        raise ValueError("credential scope mismatch")
    username = (controller / "username").read_bytes()
    secret = (controller / "secret").read_bytes()
    if (not username or not secret or any(c in username or c in secret for c in (b"\n", b"\r", b"\0"))
            or (b"username" in fields and fields[b"username"] != username)):
        raise ValueError("invalid credentials")
    return b"username=" + username + b"\npassword=" + secret + b"\n\n"


def ensure_controller_protected(controller: pathlib.Path) -> None:
    # The OCI adapter mounts this exact job-private child read-only. A distinct
    # UID alone would not stop replacement through a writable parent directory.
    if (controller.is_symlink() or not controller.is_dir()
            or not os.statvfs(controller).f_flag & os.ST_RDONLY):
        raise ValueError("controller protection required")
    for item in controller.iterdir():
        if not item.is_file() or item.is_symlink():
            raise ValueError("controller protection required")


def run(controller: pathlib.Path, checkout: pathlib.Path) -> None:
    if controller != pathlib.Path("/job/controller") or checkout != pathlib.Path("/job/project"):
        raise ValueError("invalid sandbox paths")
    ensure_controller_protected(controller)
    remote = (controller / "remote").read_text(encoding="utf-8")
    reference = (controller / "reference").read_text(encoding="utf-8")
    transport = remote_transport(remote)
    auth = (controller / "auth").read_text(encoding="ascii")
    if (transport, auth) not in (("https", "anonymous"), ("https", "https"), ("https", "bearer"), ("ssh", "ssh")):
        raise ValueError("unsupported authentication")
    if reference and (reference.startswith("-") or any(ord(c) < 33 or ord(c) == 127 for c in reference)):
        raise ValueError("invalid reference")
    with tempfile.TemporaryDirectory(prefix="piping-acquire-") as home:
        commit = acquire(controller, checkout, remote, reference, auth, home)
        emit_response(controller, checkout, commit)


def acquire(controller: pathlib.Path, checkout: pathlib.Path, remote: str, reference: str, auth: str, home: str) -> str:
    env = {
        "PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": home, "LC_ALL": "C",
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_TERMINAL_PROMPT": "0", "GIT_LFS_SKIP_SMUDGE": "1",
    }
    if auth == "ssh":
        env.update({"GIT_SSH": "/job/controller/ssh", "GIT_SSH_VARIANT": "ssh"})
        alias = (controller / "ssh-alias").read_text(encoding="ascii")
        if not re.fullmatch(r"piping-acquire-[0-9]+", alias):
            raise ValueError("invalid route alias")
        if "://" in remote:
            url = urllib.parse.urlsplit(remote)
            remote = urllib.parse.urlunsplit(("ssh", alias, url.path, "", ""))
        else:
            # Preserve scp-style relative versus absolute repository paths.
            endpoint = remote.split("@", 1)[1]
            path = endpoint.split("]:", 1)[1] if endpoint.startswith("[") else endpoint.split(":", 1)[1]
            remote = alias + ":" + path
    base = ["/usr/bin/git", "-c", "core.hooksPath=/dev/null", "-c", "http.followRedirects=false",
            "-c", "http.sslVerify=true", "-c", "credential.helper=", "-c", "protocol.allow=never",
            "-c", "protocol.https.allow=always", "-c", "protocol.ssh.allow=always"]
    if auth == "https":
        base += ["-c", "credential.useHttpPath=true", "-c", "credential.helper=/job/controller/credential-helper"]
    elif auth == "bearer":
        base += ["-c", "include.path=/job/controller/http-auth.conf"]

    def git(*args: str) -> bytes:
        process = subprocess.Popen(base + list(args), env=env, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        captured = bytearray()

        def drain():
            while chunk := process.stderr.read(4096):
                captured.extend(chunk[:max(0, 65536 - len(captured))])

        reader = threading.Thread(target=drain)
        reader.start()
        output = process.stdout.read()
        status = process.wait()
        reader.join()
        if status:
            # Never interpolate native Git diagnostics: they may include URLs,
            # server-controlled text, or credentials. Emit only fixed categories.
            diagnostic = bytes(captured).lower()
            category = ("host key verification failed" if b"host key verification failed" in diagnostic
                        else "authentication failed" if b"authentication failed" in diagnostic or b"permission denied" in diagnostic
                        else "reference not found" if b"couldn't find remote ref" in diagnostic or b"not our ref" in diagnostic or b"needed a single revision" in diagnostic
                        else "git command failed")
            raise ValueError(category)
        return output

    git("clone", "--no-checkout", "--", remote, str(checkout))
    if reference:
        git("check-ref-format", "--allow-onelevel", reference)
        git("-C", str(checkout), "fetch", "--no-tags", "--", "origin", reference)
    revision = ("FETCH_HEAD" if reference else "HEAD") + "^{commit}"
    commit = git("-C", str(checkout), "rev-parse", "--verify", "--end-of-options", revision).strip()
    if not re.fullmatch(rb"[0-9a-f]{40}|[0-9a-f]{64}", commit):
        raise ValueError("invalid commit")
    tree = git("-C", str(checkout), "ls-tree", "-rz", "--full-tree", commit.decode("ascii"))
    if any(record.startswith(b"160000 ") for record in tree.split(b"\0")):
        raise ValueError("submodules are unsupported")
    git("-C", str(checkout), "checkout", "--detach", commit.decode("ascii"), "--")
    return commit.decode("ascii")


def emit_response(controller: pathlib.Path, checkout: pathlib.Path, commit: str) -> None:
    entries = []
    total = 0
    secrets = [(controller / "secret").read_bytes()] if (controller / "secret").is_file() else []
    secrets.extend(path.read_bytes() for path in controller.glob("ssh-acquire-*.key"))

    def visit(directory, depth=0):
        nonlocal total
        if depth > 64:
            raise ValueError("tree depth exceeded")
        for item in sorted(os.scandir(directory), key=lambda item: os.fsencode(item.name)):
            if item.name == ".git" and pathlib.Path(directory) == checkout:
                continue
            if item.name in (".git", ".gitmodules"):
                raise ValueError("unsupported Git tree")
            path = pathlib.Path(item.path)
            relative = path.relative_to(checkout).as_posix()
            relative.encode("utf-8", errors="strict")
            metadata = item.stat(follow_symlinks=False)
            if stat.S_ISLNK(metadata.st_mode):
                target = os.readlink(path)
                if os.path.isabs(target) or not path.resolve(strict=False).is_relative_to(checkout):
                    raise ValueError("escaping link")
                content = {"kind": "symlink", "target": target}
            elif stat.S_ISDIR(metadata.st_mode):
                visit(path, depth + 1)
                continue
            elif stat.S_ISREG(metadata.st_mode):
                total += metadata.st_size
                if metadata.st_size > 2 * 1024 * 1024 or total > 64 * 1024 * 1024:
                    raise ValueError("snapshot size exceeded")
                data = path.read_bytes()
                if data.startswith(b"version https://git-lfs.github.com/spec/v1") or any(secret and secret in data for secret in secrets):
                    raise ValueError("unsupported or credential-bearing tree")
                content = {"kind": "file", "bytes": list(data), "executable": bool(metadata.st_mode & 0o100)}
            else:
                raise ValueError("unsupported entry")
            entries.append({"path": relative, "content": content})
            if len(entries) > 2048:
                raise ValueError("snapshot count exceeded")

    visit(checkout)
    digest = hashlib.sha256()
    digest.update(len(entries).to_bytes(8, "little"))
    for entry in sorted(entries, key=lambda entry: entry["path"].encode("utf-8")):
        path = entry["path"].encode("utf-8")
        digest.update(len(path).to_bytes(8, "little"))
        digest.update(path)
        content = entry["content"]
        if content["kind"] == "file":
            data = bytes(content["bytes"])
            digest.update(bytes((0, int(content["executable"]))))
        else:
            data = content["target"].encode("utf-8")
            digest.update(b"\x01")
        digest.update(len(data).to_bytes(8, "little"))
        digest.update(data)
    response = json.loads((controller / "request.json").read_bytes())
    response.update({"source_commit_oid": commit, "tree_sha256": digest.hexdigest(), "entries": entries})
    encoded = json.dumps(response, ensure_ascii=True, separators=(",", ":")).encode("utf-8")
    if any(secret and (secret in encoded or secret in json.dumps(response, ensure_ascii=False).encode("utf-8")) for secret in secrets):
        raise ValueError("credential-bearing response")
    if len(encoded) > 16 * 1024 * 1024:
        raise ValueError("acquisition response exceeded output bound")
    sys.stdout.buffer.write(encoded)
    sys.stdout.buffer.flush()


if __name__ == "__main__":
    try:
        if pathlib.Path(sys.argv[0]).name == "credential-helper":
            if len(sys.argv) != 2:
                raise ValueError("expected credential operation")
            sys.stdout.buffer.write(credential_response(pathlib.Path(__file__).parent, sys.argv[1], sys.stdin.buffer.read(65537)))
        else:
            if len(sys.argv) != 3:
                raise ValueError("expected controller and checkout paths")
            run(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]))
    except Exception as error:
        if pathlib.Path(sys.argv[0]).name != "credential-helper":
            safe = {"host key verification failed", "authentication failed", "git command failed",
                    "reference not found"}
            message = str(error)
            print(message if message in safe else "acquisition failed", file=sys.stderr)
        sys.exit(1)
