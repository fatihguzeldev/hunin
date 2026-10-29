import assert from 'node:assert/strict';
import {
  appendFileSync,
  readdirSync,
  readFileSync,
  statSync,
  truncateSync,
  writeFileSync,
} from 'node:fs';
import { join } from 'node:path';
import * as h from '../index.mjs';

const mode = process.argv[2];
assert.ok(h.event('before-exit'));
if (mode === 'buffered' || mode === 'flushed') {
  if (mode === 'flushed') h.flush();
  assert.equal(h.lastError(), undefined);
  process.kill(process.pid, 'SIGKILL');
} else {
  h.flush();
  assert.equal(h.lastError(), undefined);
  const directory = process.env.HUNIN_RECORD_DIR;
  assert.ok(directory);
  const files = readdirSync(directory);
  assert.equal(files.length, 1);
  const file = join(directory, files[0]!);
  const row: unknown = JSON.parse(readFileSync(file, 'utf8'));
  assert.ok(typeof row === 'object' && row !== null && !Array.isArray(row));
  if (mode === 'truncated') {
    appendFileSync(file, '{"schema_version":1');
    writeFileSync(
      join(directory, 'z-later.jsonl'),
      JSON.stringify({ ...row, id: 'later-row' }) + '\n',
    );
  } else if (mode === 'legacy') {
    truncateSync(file, statSync(file).size - 1);
  } else if (mode === 'declarations') {
    const rows = Array.from({ length: 70 }, (_, index) => ({
      ...row,
      id: `record-${index}`,
      producer_id: `declared-${index}`,
      pid: 42,
    }));
    // Sort before the ordinary declarations to exercise the byte cap even
    // when the entry-count cap would otherwise omit the last declarations.
    rows.push({
      ...row,
      id: 'oversized-producer',
      producer_id: 'a'.repeat(1024),
      pid: 42,
    });
    const encoded = rows.map((value) => JSON.stringify(value));
    for (const [id, producerId, pid] of [
      ['large-pid', 'a-large-pid', '9007199254740993'],
      ['non-u64-pid', 'a-non-u64-pid', '18446744073709551616'],
    ]) {
      // Write the numeric literal without converting it through a JS Number.
      encoded.push(
        JSON.stringify({
          ...row,
          id,
          producer_id: producerId,
          pid: '__EXACT_PID_LITERAL__',
        }).replace('"pid":"__EXACT_PID_LITERAL__"', `"pid":${pid}`),
      );
    }
    writeFileSync(file, encoded.join('\n') + '\n');
  } else {
    throw Error(`Unknown SDK completion control: ${mode}`);
  }
}
