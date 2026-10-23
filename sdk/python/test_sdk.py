import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

class SdkTest(unittest.TestCase):
    def run_target(self, code, directory=None):
        env = dict(os.environ, PYTHONPATH=str(Path(__file__).parent),
                   HUNIN_RUN_ID="test" if directory else "", HUNIN_RECORD_DIR=directory or "")
        return subprocess.run([sys.executable, "-c", code], env=env, text=True, capture_output=True)

    def test_disabled_preserves_work_and_exceptions(self):
        result = self.run_target("import hunin\nassert not hunin.enabled()\nwith hunin.span('work'): hunin.counter('items',1)\n")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_async_siblings_have_independent_parents(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.run_target('''import asyncio, hunin
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
''', directory)
            self.assertEqual(result.returncode, 0, result.stderr)
            rows = [json.loads(line) for path in Path(directory).glob('*.jsonl') for line in path.read_text().splitlines()]
            spans = {row['name']: row for row in rows if row['kind'] == 'span'}
            self.assertEqual(len(spans), 4)
            for name in ('a', 'b'):
                self.assertEqual(spans[name+'-child']['parent_id'], spans[name]['id'])
                self.assertEqual(spans[name+'-child']['context_id'], spans[name]['context_id'])
            self.assertEqual(sum(row['kind'] == 'metric' for row in rows), 2)

if __name__ == '__main__':
    unittest.main()
