import test from 'node:test';
import assert from 'node:assert/strict';
import { setImmediate } from 'node:timers/promises';
import { AttemptMonitor } from '../src/analysis/monitor.js';
import { AnalysisWatch } from '../src/analysis/watch.js';

type Pending = {
  runId: string;
  id: string;
  signal: AbortSignal;
  finish: (status: string) => void;
};

function deferredReads(): Pending[] {
  const pending: Pending[] = [];
  globalThis.fetch = async (_url, request) => {
    const { operation, params } = JSON.parse(String(request?.body));
    assert.equal(operation, 'analyses.results');
    assert.ok(request?.signal);
    return new Promise((resolve) => {
      pending.push({
        runId: params.run_id,
        id: params.execution_id,
        signal: request.signal!,
        finish: (status) =>
          resolve(
            Response.json({
              result: {
                results: [
                  {
                    id: params.execution_id,
                    run_id: params.run_id,
                    status,
                    selected: false,
                  },
                ],
                next_cursor: null,
              },
            }),
          ),
      });
    });
  };
  return pending;
}

test('switching or pausing attempt lookup aborts reads without settling late results or cancelling jobs', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const original = globalThis.fetch;
  const pending = deferredReads();
  const watch = new AnalysisWatch();
  watch.track('first', 'a');
  watch.track('second', 'b');
  const updates: string[] = [];
  const monitor = new AttemptMonitor(watch, {
    updated: (runId) => updates.push(runId),
    completed: () => assert.fail('disposed reads cannot settle attempts'),
  });
  try {
    monitor.select('first');
    t.mock.timers.tick(0);
    assert.equal(pending[0]?.runId, 'first');
    monitor.select('second');
    assert.equal(pending[0]?.signal.aborted, true);
    t.mock.timers.tick(0);
    assert.equal(pending[1]?.runId, 'second');
    pending[0]!.finish('accepted');
    await setImmediate();
    assert.deepEqual(updates, []);
    assert.equal(watch.forRun('first')[0]?.state, 'pending');

    pending[1]!.finish('running');
    await setImmediate();
    assert.deepEqual(updates, ['second']);
    t.mock.timers.tick(1499);
    assert.equal(pending.length, 2);
    t.mock.timers.tick(1);
    assert.equal(pending.length, 3);
    assert.equal(pending[2]?.id, 'b');
    monitor.pause();
    assert.equal(pending[2]?.signal.aborted, true);
    pending[2]!.finish('accepted');
    await setImmediate();
    assert.deepEqual(updates, ['second']);
    assert.equal(watch.forRun('second')[0]?.state, 'running');
    t.mock.timers.tick(10_000);
    assert.equal(pending.length, 3);
  } finally {
    monitor.pause();
    globalThis.fetch = original;
  }
});

test('a forced recheck requested during a lookup runs once after an unresolved result', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const original = globalThis.fetch;
  const pending = deferredReads();
  const watch = new AnalysisWatch();
  watch.track('recording', 'attempt');
  const completed: unknown[] = [];
  const monitor = new AttemptMonitor(watch, {
    updated: () => {},
    completed: (runId, attempts) => completed.push({ runId, attempts }),
  });
  try {
    monitor.select('recording');
    t.mock.timers.tick(0);
    monitor.recheck();
    t.mock.timers.tick(0);
    assert.equal(pending.length, 1);
    pending[0]!.finish('future-unknown-state');
    await setImmediate();
    assert.equal(watch.forRun('recording')[0]?.state, 'unresolved');
    t.mock.timers.tick(0);
    assert.equal(pending.length, 2);
    pending[1]!.finish('interrupted');
    await setImmediate();
    assert.deepEqual(completed, [
      {
        runId: 'recording',
        attempts: [{ id: 'attempt', status: 'interrupted' }],
      },
    ]);
    assert.deepEqual(watch.forRun('recording'), []);
    t.mock.timers.tick(10_000);
    assert.equal(pending.length, 2);
  } finally {
    monitor.pause();
    globalThis.fetch = original;
  }
});
