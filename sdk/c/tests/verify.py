"""Check real C/C++ recording behavior without writing into the repository."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def verify(directory, probe=False):
    environment = dict(
        os.environ, HUNIN_RUN_ID="sdk-contract", HUNIN_RECORD_DIR=str(directory)
    )
    if probe:
        environment["HUNIN_TEST_MIXED_PATH_PROBE"] = "1"
    result = subprocess.run(
        [sys.argv[1]], env=environment, capture_output=True, text=True, timeout=10
    )
    assert result.returncode == 0, (
        f"exit={result.returncode} (0x{result.returncode & 0xFFFFFFFF:08x}): {result.stderr}"
    )
    if probe:
        assert "legacy mixed-separator fopen " in result.stderr, result.stderr
        print(result.stderr.strip())
    rows = [
        json.loads(line)
        for path in Path(directory).glob("*.jsonl")
        for line in path.read_text().splitlines()
    ]
    contexts = [row for row in rows if row["kind"] == "context"]
    parents = {
        row["id"]: row
        for row in rows
        if row["kind"] == "span" and row["name"] == "parent"
    }
    children = [row for row in rows if row["kind"] == "span" and row["name"] == "child"]
    assert len(contexts) == 1 and len(parents) == len(children) == 4, rows
    identifiers = {row["id"] for row in rows}
    assert len(identifiers) == len(rows), "duplicate record IDs"
    for child in children:
        parent = parents[child["parent_id"]]
        assert child["context_id"] == parent["context_id"] == contexts[0]["id"]
        assert child["duration_us"] <= parent["duration_us"]
    for row in rows:
        assert row["run_id"] == "sdk-contract"
        for key in ("parent_id", "context_id", "from", "to"):
            if key in row:
                assert row[key] in identifiers, (key, row)
    assert len([row for row in rows if row["kind"] == "metric"]) == 4
    assert len([row for row in rows if row["kind"] == "relation"]) == 4


with tempfile.TemporaryDirectory(prefix="hunin-sdk-c-") as directory:
    verify(Path(directory))
    verify(Path(directory) / "missing" / "nested")
    if os.name == "nt":
        ordinary = Path(directory) / "extended-existing"
        ordinary.mkdir()
        absolute = os.path.abspath(ordinary)
        extended = (
            "\\\\?\\UNC\\" + absolute[2:]
            if absolute.startswith("\\\\")
            else "\\\\?\\" + absolute
        )
        verify(extended, probe=True)
        verify(extended + "\\extended-missing\\nested")
    print(
        "C/C++ contexts, spans, counters, relations and threaded flush verified for existing and nested spool directories"
    )
