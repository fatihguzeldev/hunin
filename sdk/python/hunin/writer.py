"""Process-owned producer identity, append-only wire output and sticky failures."""

import atexit
import json
import os
import threading
import time
import uuid

run_id = os.environ.get("HUNIN_RUN_ID")
_directory = os.environ.get("HUNIN_RECORD_DIR")
_producer = uuid.uuid4().hex
_lock = threading.Lock()
_fd = None
_failure = None


def _after_fork():
    global _producer, _lock, _fd, _failure
    if _fd is not None:
        os.close(_fd)
    _producer, _lock, _fd, _failure = uuid.uuid4().hex, threading.Lock(), None, None


def enabled():
    return bool(run_id and _directory and _failure is None)


def last_error():
    return _failure


def emit(kind, ambient, **fields):
    global _fd, _failure
    if not enabled():
        return None
    try:
        row = dict(
            schema_version=1,
            kind=kind,
            id=uuid.uuid4().hex,
            run_id=run_id,
            producer_id=_producer,
            pid=os.getpid(),
            tid=str(threading.get_ident()),
            timestamp_us=time.time_ns() // 1000,
            basis="explicit",
        )
        if ambient.get("context_id"):
            row["context_id"] = ambient["context_id"]
        row.update(fields)
        row["attributes"] = {
            "thread_id_source": "threading.get_ident",
            "clock": "unix_epoch",
            "clock_alignment": "direct_epoch",
            "clock_source": "time.time_ns",
            **row.get("attributes", {}),
        }
        payload = (
            json.dumps(row, allow_nan=False, separators=(",", ":")) + "\n"
        ).encode()
        with _lock:
            if _fd is None:
                os.makedirs(_directory, mode=0o700, exist_ok=True)
                _fd = os.open(
                    os.path.join(
                        _directory, "sdk-python-%s-%s.jsonl" % (os.getpid(), _producer)
                    ),
                    os.O_APPEND | os.O_WRONLY | os.O_CREAT,
                    0o600,
                )
            view = memoryview(payload)
            while view:
                view = view[os.write(_fd, view) :]
        return row["id"]
    except Exception as error:
        _failure = error
        return None


def flush():
    global _failure
    try:
        with _lock:
            if _fd is not None:
                os.fsync(_fd)
    except OSError as error:
        _failure = error


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_after_fork)

atexit.register(flush)
