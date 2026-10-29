import atexit
import contextlib
import contextvars
import json
import math
import os
import threading
import time
import uuid

_ambient = contextvars.ContextVar("hunin_context", default={})
_run = os.environ.get("HUNIN_RUN_ID")
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


if hasattr(os, "register_at_fork"):
    os.register_at_fork(after_in_child=_after_fork)


def enabled():
    return bool(_run and _directory and _failure is None)


def last_error():
    return _failure


def _emit(kind, **fields):
    global _fd, _failure
    if not enabled():
        return None
    try:
        row = dict(
            schema_version=1,
            kind=kind,
            id=uuid.uuid4().hex,
            run_id=_run,
            producer_id=_producer,
            pid=os.getpid(),
            tid=str(threading.get_ident()),
            timestamp_us=time.time_ns() // 1000,
            basis="explicit",
        )
        ambient = _ambient.get()
        if ambient.get("context_id"):
            row["context_id"] = ambient["context_id"]
        row.update(fields)
        row["attributes"] = {
            "thread_id_source": "threading.get_ident",
            "clock": "time.time_ns",
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


class Span:
    def __init__(self, name, attributes=None):
        self.name, self.attributes = name, attributes or {}
        self.id = uuid.uuid4().hex if enabled() else None
        self.started = time.perf_counter_ns()
        self.timestamp_us = time.time_ns() // 1000
        self.parent = _ambient.get().copy()
        self.ended = False
        self.owner_pid = os.getpid()
        self._token = None

    def __enter__(self):
        self._token = _ambient.set(dict(self.parent, parent_id=self.id))
        return self

    def __exit__(self, exc_type, exc, traceback):
        if self._token is not None:
            _ambient.reset(self._token)
        self.end({"status": "error", "error": str(exc)} if exc is not None else None)
        return False

    async def __aenter__(self):
        return self.__enter__()

    async def __aexit__(self, *args):
        return self.__exit__(*args)

    def end(self, attributes=None):
        if self.ended or self.id is None or self.owner_pid != os.getpid():
            return
        self.ended = True
        fields = dict(
            id=self.id,
            name=self.name,
            timestamp_us=self.timestamp_us,
            duration_us=max(0, (time.perf_counter_ns() - self.started) // 1000),
            attributes={
                **self.attributes,
                **(attributes or {}),
                "clock": "time.time_ns/perf_counter_ns",
            },
        )
        for key in ("parent_id", "context_id"):
            if self.parent.get(key):
                fields[key] = self.parent[key]
        _emit("span", **fields)


def span(name, attributes=None):
    return Span(name, attributes)


def capture_context():
    return dict(run_id=_run, **_ambient.get()) if enabled() else None


@contextlib.contextmanager
def attach_context(token):
    if not enabled() or not token or token.get("run_id") != _run:
        yield
        return
    previous = _ambient.set(
        {key: token[key] for key in ("parent_id", "context_id") if token.get(key)}
    )
    try:
        yield
    finally:
        _ambient.reset(previous)


@contextlib.contextmanager
def context(name, attributes=None):
    identifier = _emit("context", name=name, attributes=attributes or {})
    with attach_context(dict(run_id=_run, context_id=identifier)):
        yield identifier


def counter(name, value, unit="count", attributes=None):
    try:
        if not isinstance(value, (float, int)) or not math.isfinite(value):
            return None
    except (OverflowError, ValueError):
        return None
    return _emit(
        "metric",
        name=name,
        value=value,
        unit=unit,
        scope="context" if _ambient.get().get("context_id") else "thread",
        attributes=dict(attributes or {}, temporality="delta"),
    )


def event(name, attributes=None):
    return _emit("event", name=name, attributes=attributes or {})


def relation(source, target, name="links", evidence=()):
    if not source or not target:
        return None
    return _emit(
        "relation",
        **{
            "from": source,
            "to": target,
            "relation": name,
            "evidence": list(dict.fromkeys(evidence)),
        },
    )


atexit.register(flush)
