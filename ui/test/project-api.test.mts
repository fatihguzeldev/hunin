import test from 'node:test';
import assert from 'node:assert/strict';
import { operationForProject } from '../src/api.js';

test('project clients retain immutable request ownership across concurrent tabs and direct-store mode', async () => {
  const original = globalThis.fetch;
  const bodies: unknown[] = [];
  try {
    globalThis.fetch = async (_input, init) => {
      bodies.push(JSON.parse(String(init?.body)));
      return Response.json({ result: { runs: [], next_cursor: null } });
    };
    const one = operationForProject('one');
    const two = operationForProject('two');
    const direct = operationForProject();
    await Promise.all([
      one('runs.list', {}),
      two('runs.list', {}),
      direct('runs.list', {}),
      one('runs.list', {}),
    ]);
    assert.deepEqual(bodies, [
      { operation: 'runs.list', params: {}, project_id: 'one' },
      { operation: 'runs.list', params: {}, project_id: 'two' },
      { operation: 'runs.list', params: {} },
      { operation: 'runs.list', params: {}, project_id: 'one' },
    ]);
  } finally {
    globalThis.fetch = original;
  }
});
