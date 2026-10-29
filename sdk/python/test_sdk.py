import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class SdkTest(unittest.TestCase):
    def run_target(self, code, directory=None):
        env = dict(
            os.environ,
            PYTHONPATH=str(Path(__file__).parent),
            HUNIN_RUN_ID="test" if directory else "",
            HUNIN_RECORD_DIR=directory or "",
        )
        return subprocess.run(
            [sys.executable, "-c", code], env=env, text=True, capture_output=True
        )

    def test_disabled_preserves_work_and_exceptions(self):
        result = self.run_target(
            """import hunin
assert not hunin.enabled()
with hunin.span('work'):
    hunin.counter('items', 1)
class TargetError(Exception):
    calls = 0
    def __str__(self):
        self.calls += 1
        raise RuntimeError('formatter-error')
original = TargetError()
try:
    with hunin.span('disabled-error'):
        raise original
except BaseException as actual:
    assert actual is original
assert original.calls == 0
"""
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_throwing_error_formatter_preserves_sync_and_async_target_exception(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_target(
                """import asyncio, hunin
class TargetError(Exception):
    def __init__(self, failure):
        self.failure = failure
    def __str__(self):
        raise self.failure
async def main(original):
    try:
        async with hunin.span('async-error'):
            raise original
    except BaseException as actual:
        assert actual is original
for failure in (RuntimeError('formatter-error'), KeyboardInterrupt(), SystemExit(9)):
    original = TargetError(failure)
    try:
        with hunin.span('sync-error'):
            raise original
    except BaseException as actual:
        assert actual is original
    asyncio.run(main(original))
hunin.flush()
assert hunin.last_error() is None
print('target-ok')
""",
                directory,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "target-ok\n")
            rows = [
                json.loads(line)
                for path in Path(directory).glob("*.jsonl")
                for line in path.read_text().splitlines()
            ]
            self.assertEqual(len(rows), 6)
            for row in rows:
                self.assertEqual(
                    row["attributes"]["error"], "Error message unavailable"
                )

    @unittest.skipUnless(hasattr(os, "fork"), "runtime has no fork")
    def test_fork_reuses_parent_context_without_duplicating_parent_span(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_target(
                """import os, hunin
with hunin.span('parent'):
    pid = os.fork()
    if pid == 0:
        with hunin.span('child'): pass
    else:
        os.waitpid(pid, 0)
hunin.flush()
""",
                directory,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            rows = [
                json.loads(line)
                for path in Path(directory).glob("*.jsonl")
                for line in path.read_text().splitlines()
            ]
            spans = {row["name"]: row for row in rows if row["kind"] == "span"}
            self.assertEqual(len(rows), 2)
            self.assertEqual(spans["child"]["parent_id"], spans["parent"]["id"])
            self.assertNotEqual(
                spans["child"]["producer_id"], spans["parent"]["producer_id"]
            )

    def test_async_siblings_have_independent_parents(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_target(
                """import asyncio, hunin
async def work(name):
    async with hunin.span(name):
        await asyncio.sleep(0.001)
        with hunin.span(name + '-child'):
            hunin.counter('items',1)
async def main():
    with hunin.context('request'):
        await asyncio.gather(work('a'), work('b'))
asyncio.run(main())
hunin.flush()
""",
                directory,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            rows = [
                json.loads(line)
                for path in Path(directory).glob("*.jsonl")
                for line in path.read_text().splitlines()
            ]
            spans = {row["name"]: row for row in rows if row["kind"] == "span"}
            self.assertEqual(len(spans), 4)
            for name in ("a", "b"):
                self.assertEqual(spans[name + "-child"]["parent_id"], spans[name]["id"])
                self.assertEqual(
                    spans[name + "-child"]["context_id"], spans[name]["context_id"]
                )
            self.assertEqual(sum(row["kind"] == "metric" for row in rows), 2)


if __name__ == "__main__":
    unittest.main()
