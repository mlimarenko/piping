"""Job-private transport for the Kubernetes adapter, never a supervisor command."""

import errno
import json
import os
from pathlib import Path
import re
import select
import selectors
import socket
import stat
import struct
import subprocess
import sys

# Must match the supervisor's fixed NodeRestriction-protected selector key.
NODE_POOL_LABEL_KEY = "piping.space.node-restriction.kubernetes.io/ansible-pool"
CONTRACT_FIELDS = {
    "schema_version",
    "scope",
    "pids",
    "pool",
    "node_selector_key",
    "node_selector_value",
    "runtime_class",
    "native_acceptance_sha256",
}
LABEL_VALUE = re.compile(r"[A-Za-z0-9]([-A-Za-z0-9_.]{0,61}[A-Za-z0-9])?")
RUNTIME_CLASS = re.compile(r"[a-z0-9]([-a-z0-9.]{0,251}[a-z0-9])?")
DIGEST = re.compile(r"[0-9a-f]{64}")
MAX_PIDS = 4194304
CONTROL_SOCKET = "/tmp/.piping-control.sock"
READY_FILE = "/tmp/.piping-control.ready"
# Sealed structured result written by execute.py; never CRI/container logs.
NOTIFY_DIRECTORY = "/job/notify"
NOTIFY_RECORD = "result.ndjson"
NOTIFY_LIMIT = 256 * 1024
NOTIFY_FRAME = 4096


def hold():
    # PID1 must not retain CRI pipes that same-UID project code can reopen.
    null = os.open("/dev/null", os.O_RDWR)
    os.dup2(null, 1)
    os.dup2(null, 2)
    for name in os.listdir("/proc/self/fd"):
        fd = int(name)
        if fd > 2:
            try:
                os.close(fd)
            except OSError as error:
                # listdir's own descriptor is already closed when it returns.
                if error.errno != errno.EBADF:
                    raise
    # Only the wrapper's terminal status crosses this socket, never job output.
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
        server.bind(CONTROL_SOCKET)
        server.listen(1)
        Path(READY_FILE).touch(mode=0o600, exist_ok=False)
        with server.accept()[0] as connection:
            peer = connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
            pid = struct.unpack("3i", peer)[0]
            pidfd = os.pidfd_open(pid)
            status = bytearray()
            try:
                while len(status) <= 4:
                    chunk = connection.recv(5 - len(status))
                    if not chunk:
                        break
                    status.extend(chunk)
                # Keep the container alive until the exec wrapper closes its output.
                select.select([pidfd], [], [])
            finally:
                os.close(pidfd)
            if not re.fullmatch(rb"(?:0|[1-9][0-9]{0,2})\n", status):
                return 125
            code = int(status[:-1])
            return code if code <= 255 else 125


def pid_limit(value):
    return isinstance(value, int) and not isinstance(value, bool) and 0 < value <= MAX_PIDS


def validate_contract(launch):
    """Check the trusted pool metadata. It is a deployment assertion, not proof."""
    contract = launch.get("contract")
    if not isinstance(contract, dict) or set(contract) != CONTRACT_FIELDS:
        raise RuntimeError("pool contract metadata does not match schema version 1")
    version = contract["schema_version"]
    if not isinstance(version, int) or isinstance(version, bool) or version != 1:
        raise RuntimeError("unsupported pool contract schema version")
    if contract["scope"] != "pod_ancestor":
        raise RuntimeError("pool contract scope is not a Pod ancestor")
    requested = launch.get("pids")
    ceiling = contract["pids"]
    if not pid_limit(requested) or not pid_limit(ceiling) or requested != ceiling:
        raise RuntimeError("pool contract PID ceiling does not match the job request")
    if not isinstance(contract["pool"], str) or not LABEL_VALUE.fullmatch(contract["pool"]):
        raise RuntimeError("pool contract identity is malformed")
    if contract["node_selector_key"] != NODE_POOL_LABEL_KEY:
        raise RuntimeError("pool contract node selector key is not the protected label")
    if not isinstance(contract["node_selector_value"], str) or not LABEL_VALUE.fullmatch(
        contract["node_selector_value"]
    ):
        raise RuntimeError("pool contract node selector value is malformed")
    runtime_class = contract["runtime_class"]
    if runtime_class is not None and (
        not isinstance(runtime_class, str) or not RUNTIME_CLASS.fullmatch(runtime_class)
    ):
        raise RuntimeError("pool contract runtime selection is malformed")
    digest = contract["native_acceptance_sha256"]
    if not isinstance(digest, str) or not DIGEST.fullmatch(digest) or len(set(digest)) == 1:
        raise RuntimeError("pool contract native acceptance digest is malformed or a placeholder")
    return contract


def observed_pid_limits():
    limits = []
    for name in ("/sys/fs/cgroup/pids.max", "/sys/fs/cgroup/pids/pids.max"):
        try:
            path = Path(name)
            if not path.is_file():
                continue
            value = path.read_text().strip()
        except (OSError, UnicodeDecodeError):
            continue
        if value.isdigit() and 0 < int(value) <= MAX_PIDS:
            limits.append(int(value))
    return limits


def local_effective_pids(contract):
    # This is the bound visible in the calling container's own cgroup scope only.
    # A container scope reports its own larger limit or `max`; kubelet enforces
    # the Pod ancestor ceiling outside this namespace, so neither this value nor
    # the absence of a visible file verifies the contract. The guaranteed
    # ceiling stays contract["pids"].
    return min([contract["pids"], *observed_pid_limits()])


def unpack():
    payload = json.loads(Path("/bootstrap/payload.json").read_bytes())
    launch = {key: value for key, value in payload.items() if key != "tree"}
    contract = validate_contract(launch)
    # Stage-local observation of the materialisation container, not a ceiling
    # for the execution container.
    launch["materialize_local_pids"] = local_effective_pids(contract)
    root = Path("/job")
    for entry in sorted(payload["tree"], key=lambda entry: entry["path"].count("/")):
        path = root / entry["path"]
        if entry["kind"] == "directory":
            path.mkdir(mode=0o700)
    for entry in payload["tree"]:
        path = root / entry["path"]
        if entry["kind"] == "file":
            with path.open("xb") as output:
                output.write(bytes(entry["bytes"]))
            path.chmod(entry["mode"])
        elif entry["kind"] == "symlink":
            path.symlink_to(entry["target"])
    # Set restrictive directory modes only after populating their children.
    for entry in reversed(sorted(payload["tree"], key=lambda entry: entry["path"].count("/"))):
        if entry["kind"] == "directory":
            (root / entry["path"]).chmod(entry["mode"])
    path = root / ".piping-launch.json"
    with path.open("x") as output:
        json.dump(launch, output)
    path.chmod(0o600)


def notify_bytes(directory=NOTIFY_DIRECTORY):
    """Return the sealed notify payload, or None when it is absent or unsafe.

    The directory and file must both be real nodes: a symlink, a special file
    or an oversized file is refused instead of followed.
    """
    try:
        if not stat.S_ISDIR(os.lstat(directory).st_mode):
            return None
        directory_descriptor = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        return None
    try:
        try:
            descriptor = os.open(NOTIFY_RECORD, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
                                 dir_fd=directory_descriptor)
        except OSError:
            return None
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                return None
            os.set_blocking(descriptor, True)
            chunks = []
            size = 0
            while True:
                data = os.read(descriptor, NOTIFY_FRAME)
                if not data:
                    break
                size += len(data)
                if size > NOTIFY_LIMIT:
                    return None
                chunks.append(data)
            return b"".join(chunks)
        except OSError:
            return None
        finally:
            os.close(descriptor)
    finally:
        os.close(directory_descriptor)


def emit_notify(stream, directory=NOTIFY_DIRECTORY):
    """Forward the sealed result as bounded frames on the exec stdout stream."""
    try:
        payload = notify_bytes(directory)
        if not payload:
            return
        for offset in range(0, len(payload), NOTIFY_FRAME):
            frame = payload[offset:offset + NOTIFY_FRAME]
            stream.write(json.dumps({"stream": "notify", "bytes": list(frame)}, separators=(",", ":")) + "\n")
            stream.flush()
    except Exception:
        return


def run():
    launch = json.loads(Path("/job/.piping-launch.json").read_bytes())
    contract = validate_contract(launch)
    # The recorded value belongs to the materialisation container. This stage
    # neither reuses it as its own bound nor claims to observe the Pod ancestor;
    # only its shape and the contracted ceiling bound it.
    materialized = launch.get("materialize_local_pids")
    if not pid_limit(materialized) or materialized > contract["pids"]:
        raise RuntimeError("recorded materialisation PID bound is invalid")
    control = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    control.connect(CONTROL_SOCKET)
    child = subprocess.Popen(
        launch["argv"], cwd=launch["workdir"], env=launch["env"],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        start_new_session=True, close_fds=True,
    )
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ, "stdout")
    selector.register(child.stderr, selectors.EVENT_READ, "stderr")
    count = 0
    while selector.get_map():
        for key, _ in selector.select():
            # Bound each frame for the supervisor's streaming parser.
            data = os.read(key.fd, 2048)
            if not data:
                selector.unregister(key.fileobj)
                key.fileobj.close()
                continue
            count += len(data)
            if count > launch["output_limit"]:
                child.kill()
                child.wait()
                return 125
            # The exec stream carries frames directly to the supervisor, not CRI.
            sys.stdout.write(json.dumps({"stream": key.data, "bytes": list(data)}, separators=(",", ":")) + "\n")
            sys.stdout.flush()
    code = child.wait()
    code = code if code >= 0 else min(255, 128 - code)
    # The sealed structured result travels on stdout before the terminal status
    # crosses the control socket, never through stderr or the console stream.
    emit_notify(sys.stdout)
    control.sendall(f"{code}\n".encode("ascii"))
    control.shutdown(socket.SHUT_WR)
    control.close()
    # The real exit is delivered to PID1; kubectl itself must complete cleanly.
    return 0


if __name__ == "__main__":
    os.umask(0o077)
    try:
        if sys.argv[1] == "unpack":
            unpack()
        elif sys.argv[1] == "hold":
            sys.exit(hold())
        elif sys.argv[1] == "run":
            sys.exit(run())
        else:
            sys.exit(125)
    except Exception:
        # Exception messages can contain project bytes or credentials.
        sys.stderr.write("Kubernetes job transport failed\n")
        sys.exit(125)
