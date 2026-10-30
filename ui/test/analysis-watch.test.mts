import test from 'node:test';
import assert from 'node:assert/strict';
import { AnalysisWatch } from '../src/analysis/watch.js';

function session() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
}
const signal = () => new AbortController().signal;

test('known attempt handles survive reload and settle only from an exact owned terminal response', async () => {
  const storage = session();
  const first = new AnalysisWatch(storage);
  first.track('recording', 'attempt');
  first.track('other', 'attempt');
  const reload = new AnalysisWatch(storage);
  assert.equal(reload.forRun('recording')[0]?.state, 'pending');
  const completed = await reload.check(
    'recording',
    async (run, id) => [{ run_id: run, id, status: 'accepted' }],
    signal(),
  );
  assert.deepEqual(completed, [{ id: 'attempt', status: 'accepted' }]);
  assert.deepEqual(reload.forRun('recording'), []);
  assert.equal(reload.forRun('other').length, 1);
  assert.equal(reload.track('recording', 'attempt'), false); // A late running page cannot resurrect a settled attempt.
  assert.deepEqual(new AnalysisWatch(storage).forRun('recording'), []);
});

test('missing, mismatched, scalar and extension statuses remain unresolved rather than completed', async () => {
  for (const result of [
    [],
    [{ run_id: 'other', id: 'attempt', status: 'accepted' }],
    [null],
    [{ run_id: 'recording', id: 'attempt', status: 'future-status' }],
  ]) {
    const watch = new AnalysisWatch();
    watch.track('recording', 'attempt');
    assert.deepEqual(
      await watch.check('recording', async () => result, signal()),
      [],
    );
    assert.equal(watch.forRun('recording')[0]?.state, 'unresolved');
    let reads = 0;
    await watch.check(
      'recording',
      async () => {
        reads++;
        return [];
      },
      signal(),
    );
    assert.equal(reads, 0); // Do not poll an unknown state indefinitely.
    assert.deepEqual(
      await watch.check(
        'recording',
        async () => [
          { run_id: 'recording', id: 'attempt', status: 'cancelled' },
        ],
        signal(),
        true,
      ),
      [{ id: 'attempt', status: 'cancelled' }],
    );
  }
});

test('disposing lookup interest ignores a late terminal response and leaves the persisted job handle intact', async () => {
  const watch = new AnalysisWatch();
  watch.track('recording', 'attempt');
  const lifetime = new AbortController();
  let resolve: ((rows: unknown[]) => void) | undefined;
  const pending = watch.check(
    'recording',
    () =>
      new Promise((finish) => {
        resolve = finish;
      }),
    lifetime.signal,
  );
  lifetime.abort();
  assert.ok(resolve);
  resolve([{ run_id: 'recording', id: 'attempt', status: 'accepted' }]);
  assert.deepEqual(await pending, []);
  assert.equal(watch.forRun('recording')[0]?.state, 'pending');
});

test('lookup failures remain explicit and a retry reads authoritative state again', async () => {
  const watch = new AnalysisWatch();
  watch.track('recording', 'attempt');
  await watch.check(
    'recording',
    async () => {
      throw new Error('service unavailable');
    },
    signal(),
  );
  assert.match(
    watch.forRun('recording')[0]?.diagnostic ?? '',
    /service unavailable/,
  );
  await watch.check(
    'recording',
    async () => [{ run_id: 'recording', id: 'attempt', status: 'running' }],
    signal(),
    true,
  );
  assert.equal(watch.forRun('recording')[0]?.state, 'running');
});

test('session persistence stores only lookup handles and never treats a cached status as job authority', async () => {
  const storage = session();
  const watch = new AnalysisWatch(storage);
  watch.track('recording', 'attempt');
  await watch.check(
    'recording',
    async () => [
      {
        run_id: 'recording',
        id: 'attempt',
        status: 'running',
        diagnostics: ['retained private output'],
      },
    ],
    signal(),
  );
  watch.track('recording', 'second');
  const saved = JSON.parse(storage.getItem('hunin.analysis-watch.v1') ?? '{}');
  assert.deepEqual(saved, {
    version: 1,
    attempts: [
      { runId: 'recording', id: 'attempt' },
      { runId: 'recording', id: 'second' },
    ],
  });
  assert.equal(
    new AnalysisWatch(storage).forRun('recording')[0]?.status,
    undefined,
  );
  const unavailable = new AnalysisWatch({
    getItem: () => {
      throw new Error('unavailable');
    },
    setItem: () => {
      throw new Error('quota');
    },
  });
  unavailable.track('recording', 'memory-only');
  assert.equal(unavailable.persistent, false);
  assert.equal(unavailable.forRun('recording')[0]?.id, 'memory-only');
});

test('the backend recovery interrupted status is terminal without implying accepted output', async () => {
  const watch = new AnalysisWatch();
  watch.track('recording', 'abandoned-owner');
  assert.deepEqual(
    await watch.check(
      'recording',
      async () => [
        { run_id: 'recording', id: 'abandoned-owner', status: 'interrupted' },
      ],
      signal(),
    ),
    [{ id: 'abandoned-owner', status: 'interrupted' }],
  );
  assert.deepEqual(watch.forRun('recording'), []);
});

test('confirmed recording removal forgets its read interest without canceling or forgetting other jobs', () => {
  const storage = session();
  const watch = new AnalysisWatch(storage);
  watch.track('removed', 'attempt-a');
  watch.track('kept', 'attempt-b');
  watch.forgetRun('removed');
  assert.deepEqual(watch.forRun('removed'), []);
  assert.equal(watch.forRun('kept').length, 1);
  assert.equal(watch.track('removed', 'late-start-response'), false);
  const restored = new AnalysisWatch(storage);
  assert.deepEqual(restored.forRun('removed'), []);
  assert.equal(restored.forRun('kept').length, 1);
});
