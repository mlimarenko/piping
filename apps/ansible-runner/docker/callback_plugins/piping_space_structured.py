"""Bounded structured notification callback for Piping Ansible jobs.

The trusted bootstrap enables this aggregate plugin by name on top of the
project's effective callback configuration; it never replaces the configured
stdout callback. Two append-only NDJSON spools are written under
``$PIPING_NOTIFY_DIR`` and the bootstrap seals them into one result file after
Ansible exits. A spool that cannot be opened safely disables only itself: no
callback method raises into Ansible and no module result content is written.
"""

import json
import os

from ansible.plugins.callback import CallbackBase

NOTIFY_DIRECTORY_VARIABLE = "PIPING_NOTIFY_DIR"
HOSTS_RECORD = "hosts.ndjson"
TASKS_RECORD = "tasks.ndjson"
HOSTS_LIMIT = 64 * 1024
TASKS_LIMIT = 192 * 1024
LINE_LIMIT = 2048
TASK_NAME_LIMIT = 200
TRUNCATED = {"kind": "truncated"}
# Output field -> AggregateStats.summarize() key. ``summarize`` reports
# ``failures``; the record field is the contracted ``failed``.
HOST_COUNTERS = (
    ("ok", "ok"),
    ("changed", "changed"),
    ("unreachable", "unreachable"),
    ("failed", "failures"),
    ("skipped", "skipped"),
    ("rescued", "rescued"),
    ("ignored", "ignored"),
)


def _line(values):
    """Serialize one record as a single ASCII NDJSON line, or nothing."""
    try:
        return json.dumps(values, separators=(",", ":"), ensure_ascii=True).encode("ascii") + b"\n"
    except (TypeError, ValueError):
        return None


def _count(value):
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        return 0
    return value


class _Spool:
    """One bounded NDJSON file, opened lazily and disabled on any doubt."""

    def __init__(self, directory, name, limit):
        self._path = os.path.join(directory, name)
        self._limit = limit
        self._descriptor = None
        self._opened = False
        self._written = 0
        self._stopped = False

    def record(self, values):
        if self._stopped or not self._start():
            return
        line = _line(values)
        if line is None or len(line) > LINE_LIMIT or self._written + len(line) > self._limit:
            self._truncate()
            return
        self._append(line)

    def _start(self):
        if self._opened:
            return self._descriptor is not None
        self._opened = True
        try:
            self._descriptor = os.open(
                self._path,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                0o600,
            )
        except OSError:
            self._descriptor = None
        return self._descriptor is not None

    def _append(self, line):
        try:
            offset = 0
            while offset < len(line):
                written = os.write(self._descriptor, line[offset:])
                if written <= 0:
                    raise OSError("no progress")
                offset += written
            self._written += len(line)
        except OSError:
            self._stop()

    def _truncate(self):
        marker = _line(TRUNCATED)
        if (
            marker is not None
            and len(marker) <= LINE_LIMIT
            and self._start()
            and self._written + len(marker) <= self._limit
        ):
            self._append(marker)
        self._stop()

    def _stop(self):
        self._stopped = True
        descriptor, self._descriptor = self._descriptor, None
        if descriptor is not None:
            try:
                os.close(descriptor)
            except OSError:
                pass


class CallbackModule(CallbackBase):
    CALLBACK_VERSION = 2.0
    CALLBACK_TYPE = "aggregate"
    CALLBACK_NAME = "piping_space_structured"
    CALLBACK_NEEDS_ENABLED = True

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        directory = os.environ.get(NOTIFY_DIRECTORY_VARIABLE) or ""
        if directory:
            self._hosts = _Spool(directory, HOSTS_RECORD, HOSTS_LIMIT)
            self._tasks = _Spool(directory, TASKS_RECORD, TASKS_LIMIT)
        else:
            self._hosts = None
            self._tasks = None

    def v2_playbook_on_stats(self, stats):
        try:
            spool = self._hosts
            if spool is None:
                return
            hosts = [host for host in stats.processed if isinstance(host, str)]
            for host in sorted(hosts):
                summary = stats.summarize(host)
                record = {"host": host}
                for field, source in HOST_COUNTERS:
                    record[field] = _count(summary.get(source))
                spool.record(record)
        except Exception:
            return

    def v2_runner_on_ok(self, result):
        try:
            changed = bool(result._result.get("changed", False))
        except Exception:
            changed = False
        self._task(result, "changed" if changed else "ok")

    def v2_runner_on_failed(self, result, ignore_errors=False):
        self._task(result, "ignored" if ignore_errors else "failed")

    def v2_runner_on_unreachable(self, result):
        self._task(result, "unreachable")

    def v2_runner_on_skipped(self, result):
        self._task(result, "skipped")

    def _task(self, result, outcome):
        try:
            spool = self._tasks
            if spool is None:
                return
            host = result._host.get_name()
            name = result._task.get_name()
            if not isinstance(host, str) or not isinstance(name, str):
                return
            spool.record({"host": host, "task": name[:TASK_NAME_LIMIT], "result": outcome})
        except Exception:
            return
