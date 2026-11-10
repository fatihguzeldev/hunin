import json
import os
import socket
import struct
import sys
import time


def read_exact(stream, count):
    chunks = []
    while count:
        part = stream.recv(count)
        if not part:
            raise RuntimeError("core closed before acknowledgment")
        chunks.append(part)
        count -= len(part)
    return b"".join(chunks)


def exchange(stream, message):
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    stream.sendall(struct.pack(">I", len(body)) + body)
    size = struct.unpack(">I", read_exact(stream, 4))[0]
    if size > 1024 * 1024:
        raise RuntimeError("acknowledgment exceeds limit")
    return json.loads(read_exact(stream, size))


host, port, project, source, module = sys.argv[1:]
with socket.create_connection((host, int(port)), timeout=3) as stream:
    exchange(stream, {"type": "hello", "version": 1, "project": project,
                      "source": source, "credential": os.environ["HUNIN_PRODUCER_CREDENTIAL"],
                      "required_semantics": ["structured-records-v1"]})
    exchange(stream, {"type": "ready", "coverage": {"method": "filesystem metadata read"}})
    before = time.monotonic_ns()
    size = os.stat(module).st_size
    after = time.monotonic_ns()
    result = exchange(stream, {"type": "batch", "delivery": "stat-delivery-1", "records": [{
        "id": "stat-1", "schema": "external.file-stat", "schema_version": 1,
        "subject": "prepared-module", "time": {"clock": "driver-monotonic",
            "start": before, "end": after, "uncertainty": 0},
        "function": None, "location": None, "operation": "preview-operation",
        "payload": {"value": size, "path": module}}]})
    if not result["durable"] or result["publication"]["inserted"] != 1:
        raise RuntimeError("measurement was not durably published")
    exchange(stream, {"type": "finish", "outcome": {"complete": True}})
