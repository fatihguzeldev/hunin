"""Context propagation and explicit observation/span lifetimes."""

import contextlib
import contextvars
import math
import os
import time
import uuid

from . import writer

_ambient = contextvars.ContextVar("hunin_context", default={})


def _error_message(error):
    try:
        return str(error)
    except BaseException:
        # Diagnostic formatting must not replace the active target exception.
        return "Error message unavailable"


class Span:
    def __init__(self, name, attributes=None):
        self.name, self.attributes = name, attributes or {}
        self.id = uuid.uuid4().hex if writer.enabled() else None
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
        attributes = None
        if (
            exc is not None
            and self.id is not None
            and not self.ended
            and self.owner_pid == os.getpid()
            and writer.enabled()
        ):
            attributes = {"status": "error", "error": _error_message(exc)}
        self.end(attributes)
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
        writer.emit("span", _ambient.get(), **fields)


def span(name, attributes=None):
    return Span(name, attributes)


def capture_context():
    return dict(run_id=writer.run_id, **_ambient.get()) if writer.enabled() else None


@contextlib.contextmanager
def attach_context(token):
    if not writer.enabled() or not token or token.get("run_id") != writer.run_id:
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
    identifier = writer.emit(
        "context", _ambient.get(), name=name, attributes=attributes or {}
    )
    with attach_context(dict(run_id=writer.run_id, context_id=identifier)):
        yield identifier


def counter(name, value, unit="count", attributes=None):
    try:
        if (
            isinstance(value, bool)
            or not isinstance(value, (float, int))
            or not math.isfinite(value)
        ):
            return None
    except (OverflowError, ValueError):
        return None
    return writer.emit(
        "metric",
        _ambient.get(),
        name=name,
        value=value,
        unit=unit,
        scope="context" if _ambient.get().get("context_id") else "thread",
        attributes=dict(attributes or {}, temporality="delta"),
    )


def event(name, attributes=None):
    return writer.emit("event", _ambient.get(), name=name, attributes=attributes or {})


def relation(source, target, name="links", evidence=()):
    if not source or not target:
        return None
    return writer.emit(
        "relation",
        _ambient.get(),
        **{
            "from": source,
            "to": target,
            "relation": name,
            "evidence": list(dict.fromkeys(evidence)),
        },
    )
