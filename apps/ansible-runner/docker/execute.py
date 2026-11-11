#!/usr/bin/env python3
"""Trusted OCI job bootstrap. All arguments are separate argv elements."""
import argparse
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import threading

NEEDS_INPUT = 78
PROMPTS = {"-k", "-K", "-J", "--ask-pass", "--ask-become-pass", "--ask-vault-password", "--ask-vault-pass", "--step"}
EOF_ERRORS = (b"EOFError: EOF when reading a line", b"EOF when reading a line", b"input stream is not interactive")

# The trusted callback directory and spool are provisioned by this bootstrap
# inside the job container; no project file is trusted to declare them.
NOTIFY_DIRECTORY = "/job/notify"
CALLBACK_DIRECTORY = "/job/controller/callback_plugins"
CALLBACK_NAME = "piping_space_structured"
HOSTS_SPOOL = "hosts.ndjson"
TASKS_SPOOL = "tasks.ndjson"
RESULT_RECORD = "result.ndjson"
SPOOL_LINE_LIMIT = 2048
SPOOL_LIMIT = 256 * 1024
HOST_FIELDS = ("host", "ok", "changed", "unreachable", "failed", "skipped", "rescued", "ignored")
COUNTER_FIELDS = HOST_FIELDS[1:]
TASK_RESULTS = frozenset({"ok", "changed", "failed", "ignored", "skipped", "unreachable"})


def execute(argv, env, cwd):
    """Forward native bytes while keeping a bounded stderr tail for EOF diagnosis."""
    process = subprocess.Popen(argv, env=env, cwd=cwd, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    tail = bytearray()

    def forward(source, destination, capture):
        while data := source.read1(16384):
            destination.write(data)
            destination.flush()
            if capture:
                tail.extend(data)
                if len(tail) > 65536:
                    del tail[:-65536]

    threads = [threading.Thread(target=forward, args=(process.stdout, sys.stdout.buffer, False)),
               threading.Thread(target=forward, args=(process.stderr, sys.stderr.buffer, True))]
    for thread in threads:
        thread.start()
    code = process.wait()
    for thread in threads:
        thread.join()
    if code and any(message in tail for message in EOF_ERRORS):
        return NEEDS_INPUT
    return code if code >= 0 else 128 - code


def effective_config(env, project, required):
    """Read the effective Ansible configuration, or nothing when it is optional."""
    try:
        result = subprocess.run(["ansible-config", "dump", "--format", "json"], cwd=project,
                                env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, check=True)
        entries = json.loads(result.stdout)
        if not isinstance(entries, list):
            raise ValueError("Ansible returned an invalid configuration dump")
        return entries
    except (OSError, subprocess.CalledProcessError, ValueError):
        if required:
            raise
        return None


def prepare_spool(env):
    """Create the private notify directory and drop any stale records.

    The variable is exported for every job; the callback plugin is only enabled
    when this preparation succeeds.
    """
    env["PIPING_NOTIFY_DIR"] = NOTIFY_DIRECTORY
    try:
        try:
            status = os.lstat(NOTIFY_DIRECTORY)
        except FileNotFoundError:
            status = None
        if status is not None and not stat.S_ISDIR(status.st_mode):
            # A symlink or plain file in that place is never trusted.
            os.unlink(NOTIFY_DIRECTORY)
            status = None
        if status is None:
            os.mkdir(NOTIFY_DIRECTORY, 0o700)
        # The process umask must not decide the spool mode.
        os.chmod(NOTIFY_DIRECTORY, 0o700)
        for name in (HOSTS_SPOOL, TASKS_SPOOL, RESULT_RECORD):
            try:
                os.unlink(os.path.join(NOTIFY_DIRECTORY, name))
            except FileNotFoundError:
                continue
    except OSError:
        return False
    return True


def _unique(values):
    seen = []
    for value in values:
        if value not in seen:
            seen.append(value)
    return seen


def callback_environment(env, config):
    """Extend the effective callback configuration with the trusted plugin.

    Every effective entry is preserved and deduplicated; the configured stdout
    callback and Ansible's argv are never touched.
    """
    paths = config.get("DEFAULT_CALLBACK_PLUGIN_PATH")
    enabled = config.get("CALLBACKS_ENABLED")
    if not all(isinstance(value, list) and all(isinstance(item, str) for item in value)
               for value in (paths, enabled)):
        return False
    env["ANSIBLE_CALLBACK_PLUGINS"] = os.pathsep.join(_unique([CALLBACK_DIRECTORY, *paths]))
    env["ANSIBLE_CALLBACKS_ENABLED"] = ",".join(_unique([*enabled, CALLBACK_NAME]))
    return True


def _line(values):
    return json.dumps(values, separators=(",", ":"), ensure_ascii=True).encode("ascii") + b"\n"


def _text(value):
    if not isinstance(value, str) or not value:
        return None
    return None if any(character < " " or character == "\x7f" for character in value) else value


def _counter(value):
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        return None
    return value


def _host_fields(value):
    host = _text(value.get("host"))
    if host is None:
        return None
    fields = {"host": host}
    for name in COUNTER_FIELDS:
        count = _counter(value.get(name))
        if count is None:
            return None
        fields[name] = count
    return fields


def _task_fields(value):
    host = _text(value.get("host"))
    name = _text(value.get("task"))
    outcome = value.get("result")
    if host is None or name is None or outcome not in TASK_RESULTS:
        return None
    return {"host": host, "task": name, "result": outcome}


def _canonical(validate, kind, line):
    """Return one canonical result line, or None for an invalid spool line."""
    if len(line) > SPOOL_LINE_LIMIT or not line.endswith(b"\n"):
        return None
    try:
        value = json.loads(line)
    except ValueError:
        return None
    if not isinstance(value, dict):
        return None
    fields = validate(value)
    if fields is None:
        return None
    return _line({"kind": kind, **fields})


def _spool_records():
    """Validate both spools into result lines; None when neither file exists."""
    marker = _line({"kind": "truncated"})
    budget = SPOOL_LIMIT - len(marker)
    records = []
    published = 0
    truncated = False
    total = 0
    present = False
    for name, validate, kind in ((HOSTS_SPOOL, _host_fields, "host"),
                                 (TASKS_SPOOL, _task_fields, "task")):
        path = os.path.join(NOTIFY_DIRECTORY, name)
        try:
            os.lstat(path)
        except FileNotFoundError:
            continue
        except OSError:
            truncated = True
            continue
        present = True
        try:
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        except OSError:
            truncated = True
            continue
        with os.fdopen(descriptor, "rb") as stream:
            while True:
                line = stream.readline(SPOOL_LINE_LIMIT + 1)
                if not line:
                    break
                total += len(line)
                if total > SPOOL_LIMIT:
                    truncated = True
                    break
                record = _canonical(validate, kind, line)
                if record is None:
                    truncated = True
                    continue
                if published + len(record) > budget:
                    truncated = True
                    break
                records.append(record)
                published += len(record)
    if not present:
        return None
    if truncated:
        records.append(marker)
    return b"".join(records)


def _write_result(payload):
    path = os.path.join(NOTIFY_DIRECTORY, RESULT_RECORD)
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    except OSError:
        return
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
    except OSError:
        try:
            os.unlink(path)
        except OSError:
            pass


def seal_spool():
    """Publish the sealed structured result; never raises and never prints."""
    try:
        payload = _spool_records()
        if payload is not None:
            _write_result(payload)
    except Exception:
        return


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", required=True)
    parser.add_argument("--home", required=True)
    parser.add_argument("--requirements", action="append", default=[])
    parser.add_argument("--role-requirements", action="append", default=[])
    parser.add_argument("--collection-requirements", action="append", default=[])
    parser.add_argument("--record-ssh-config")
    parser.add_argument("--record-ssh-key-file")
    parser.add_argument("ansible_args", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if bool(args.record_ssh_config) != bool(args.record_ssh_key_file):
        parser.error("record SSH config and key file must be supplied together")
    if args.record_ssh_config and not all(Path(p).is_absolute() and Path(p).is_file()
                                          for p in (args.record_ssh_config, args.record_ssh_key_file)):
        parser.error("record SSH paths must be absolute files")
    project_arg = Path(args.project)
    project = project_arg.resolve(strict=True)
    home = Path(args.home)
    if not project_arg.is_absolute() or not home.is_absolute() or not project.is_dir():
        parser.error("project and home must be absolute job paths")
    argv = args.ansible_args
    if argv and argv[0] == "--":
        argv = argv[1:]
    if not argv:
        parser.error("missing playbook arguments")
    for index, arg in enumerate(argv):
        name = arg.split("=", 1)[0]
        short_with_value = len(arg) > 2 and arg[0] == "-" and arg[1] in "ielucTtfM"
        if name in PROMPTS or (arg.startswith("-") and not arg.startswith("--") and not short_with_value and len(arg) > 2 and any(c in arg[1:] for c in "kKJ")):
            print("Manual input is unsupported", file=sys.stderr)
            return NEEDS_INPUT
        if name == "--vault-id":
            value = arg.split("=", 1)[1] if "=" in arg else (argv[index + 1] if index + 1 < len(argv) else "")
            if value == "prompt" or value.endswith("@prompt"):
                print("Manual input is unsupported", file=sys.stderr)
                return NEEDS_INPUT
    home.mkdir(mode=0o700, parents=True, exist_ok=True)
    env = dict(os.environ)
    env["HOME"] = str(home)
    # Ask Ansible itself for effective cfg/environment paths before prepending
    # the job-private install locations. Never replace the configured paths.
    roles = home / ".ansible" / "roles"
    collections = home / ".ansible" / "collections"
    requirements = [(None, value, None) for value in args.requirements]
    requirements += [("role", value, roles) for value in args.role_requirements]
    requirements += [("collection", value, collections) for value in args.collection_requirements]
    # The dump supplies both the search paths and the effective callback
    # configuration. Requirements and the SSH record need it and already failed
    # without it; another job only loses structured reporting.
    entries = effective_config(env, project, bool(requirements or args.record_ssh_config))
    config = {}
    if entries is not None:
        if args.record_ssh_config:
            transports = [entry for entry in entries
                          if isinstance(entry, dict) and entry.get("name") == "DEFAULT_TRANSPORT"]
            if len(transports) != 1 or not isinstance(transports[0].get("origin"), str):
                raise ValueError("Ansible returned invalid transport origin")
            env["PIPING_RECORD_SSH_CONFIG"] = args.record_ssh_config
            env["PIPING_RECORD_SSH_KEY_FILE"] = args.record_ssh_key_file
            if transports[0]["origin"] == "default":
                env["ANSIBLE_TRANSPORT"] = "piping.space.record_ssh"
        config = {entry["name"]: entry["value"] for entry in entries
                  if isinstance(entry, dict) and "name" in entry and "value" in entry}
    if requirements:
        for key, variable, private in (("DEFAULT_ROLES_PATH", "ANSIBLE_ROLES_PATH", roles),
                                       ("COLLECTIONS_PATHS", "ANSIBLE_COLLECTIONS_PATH", collections)):
            paths = config[key]
            if not isinstance(paths, list) or not all(isinstance(path, str) for path in paths):
                raise ValueError("Ansible returned invalid search paths")
            env[variable] = os.pathsep.join([str(private), *paths])
        for kind, value, destination in requirements:
            requirement = (project / value).resolve(strict=True)
            if not requirement.is_relative_to(project) or not requirement.is_file():
                raise ValueError("requirements must be files inside the project")
            if kind is None:
                roles.mkdir(parents=True, exist_ok=True)
                collections.mkdir(parents=True, exist_ok=True)
                command = ["ansible-galaxy", "install", "-r", str(requirement)]
            else:
                destination.mkdir(parents=True, exist_ok=True)
                command = ["ansible-galaxy", kind, "install", "-r", str(requirement), "-p", str(destination)]
            code = execute(command, env, project)
            if code:
                return code
    # Structured notifications ride on the effective configuration instead of
    # replacing it. A dump that is optional and unavailable leaves the job
    # unchanged and its structured data unreported.
    notify = prepare_spool(env)
    if notify and entries is not None:
        notify = callback_environment(env, config)
    code = execute(["ansible-playbook", *argv], env, project)
    if notify:
        seal_spool()
    return code


if __name__ == "__main__":
    sys.exit(main())
