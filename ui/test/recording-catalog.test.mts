import test from 'node:test';
import assert from 'node:assert/strict';
import { RecordingCatalog } from '../src/recordings/catalog.js';
import type { Run } from '../src/api.js';

function run(id: string): Run {
  return {
    id,
    name: id,
    status: 'completed',
    command: [],
    cwd: '/fixture',
    runtime: 'imported',
    started_at_us: 1,
    finished_at_us: 2,
    exit_code: 0,
    diagnostics: [],
    source: {},
    correctness: null,
  };
}

function deferredReads() {
  const pending: {
    signal: AbortSignal;
    cursor?: string;
    finish: (runs: Run[], cursor?: string) => void;
    fail: (error: Error) => void;
  }[] = [];
  globalThis.fetch = async (_url, request) => {
    const { operation, params } = JSON.parse(String(request?.body));
    assert.equal(operation, 'runs.list');
    assert.ok(request?.signal);
    return new Promise((resolve, reject) => {
      pending.push({
        signal: request.signal!,
        cursor: params.cursor,
        finish: (runs, cursor) =>
          resolve(
            Response.json({ result: { runs, next_cursor: cursor ?? null } }),
          ),
        fail: reject,
      });
    });
  };
  return pending;
}

test('catalog refresh retains the current selection and merges older pages without replacing known records', async () => {
  const original = globalThis.fetch;
  const pending = deferredReads();
  const catalog = new RecordingCatalog();
  let selected = 'first';
  try {
    const initial = catalog.refresh(() => selected);
    pending[0]!.finish([run('first'), run('selected')], 'older');
    assert.equal((await initial).current(), true);
    const refresh = catalog.refresh(() => selected);
    selected = 'selected'; // Navigation can change while the list request is pending.
    pending[1]!.finish([run('newest')], 'older');
    await refresh;
    assert.deepEqual(
      catalog.runs.map((row) => row.id),
      ['newest', 'selected'],
    );
    const older = catalog.older();
    assert.equal(pending[2]?.cursor, 'older');
    pending[2]!.finish([
      { ...run('selected'), name: 'duplicate page row' },
      run('oldest'),
    ]);
    await older;
    assert.deepEqual(
      catalog.runs.map((row) => row.id),
      ['newest', 'selected', 'oldest'],
    );
    assert.equal(catalog.runs[1]?.name, 'selected');
    assert.equal(catalog.hasOlder, false);
  } finally {
    catalog.pause();
    globalThis.fetch = original;
  }
});

test('replacing or disposing a catalog read ignores late data and preserves its last accepted pages', async () => {
  const original = globalThis.fetch;
  const pending = deferredReads();
  const catalog = new RecordingCatalog();
  try {
    const obsolete = catalog.refresh(() => '');
    const current = catalog.refresh(() => '');
    assert.equal(pending[0]?.signal.aborted, true);
    pending[1]!.finish([run('current')], 'older');
    const currentRead = await current;
    pending[0]!.finish([run('obsolete')]);
    assert.equal((await obsolete).current(), false);
    assert.deepEqual(
      catalog.runs.map((row) => row.id),
      ['current'],
    );
    const older = catalog.older();
    catalog.pause();
    assert.equal(pending[2]?.signal.aborted, true);
    assert.equal(currentRead.current(), false);
    pending[2]!.finish([run('disposed')]);
    assert.equal((await older).current(), false);
    assert.deepEqual(
      catalog.runs.map((row) => row.id),
      ['current'],
    );
  } finally {
    catalog.pause();
    globalThis.fetch = original;
  }
});

test('catalog failures carry their exact read lifetime so an obsolete failure cannot replace the active view', async () => {
  const original = globalThis.fetch;
  const pending = deferredReads();
  const catalog = new RecordingCatalog();
  try {
    const failed = catalog.refresh(() => '');
    const failure = new Error('list unavailable');
    pending[0]!.fail(failure);
    const read = await failed;
    assert.equal(read.error, failure);
    assert.equal(read.current(), true);
    const current = catalog.refresh(() => '');
    assert.equal(read.current(), false);
    pending[1]!.finish([run('current')]);
    await current;
    assert.deepEqual(
      catalog.runs.map((row) => row.id),
      ['current'],
    );
  } finally {
    catalog.pause();
    globalThis.fetch = original;
  }
});

test('simultaneous pagination consumers share one page and advance its cursor once', async () => {
  const original = globalThis.fetch;
  const pending = deferredReads();
  const catalog = new RecordingCatalog();
  try {
    assert.equal((await catalog.older()).current(), false);
    assert.equal(pending.length, 0);
    const initial = catalog.refresh(() => '');
    pending[0]!.finish([run('first')], 'page-two');
    await initial;

    const sidebar = catalog.older();
    const library = catalog.older();
    assert.equal(catalog.loadingOlder, true);
    assert.equal(pending.length, 2);
    assert.equal(pending[1]?.cursor, 'page-two');
    pending[1]!.finish([run('second')], 'page-three');
    assert.equal((await sidebar).current(), true);
    assert.equal((await library).current(), true);
    assert.equal(catalog.loadingOlder, false);

    const next = catalog.older();
    assert.equal(pending[2]?.cursor, 'page-three');
    pending[2]!.finish([run('third')]);
    await next;
    assert.deepEqual(
      catalog.runs.map((row) => row.id),
      ['first', 'second', 'third'],
    );
    assert.equal((await catalog.older()).current(), false);
    assert.equal(pending.length, 3);
  } finally {
    catalog.pause();
    globalThis.fetch = original;
  }
});

test('failed shared pagination remains retryable at the same cursor', async () => {
  const original = globalThis.fetch;
  const pending = deferredReads();
  const catalog = new RecordingCatalog();
  try {
    const initial = catalog.refresh(() => '');
    pending[0]!.finish([run('first')], 'older');
    await initial;
    const first = catalog.older();
    const second = catalog.older();
    assert.equal(pending.length, 2);
    const failure = new Error('page unavailable');
    pending[1]!.fail(failure);
    assert.equal((await first).error, failure);
    assert.equal((await second).error, failure);
    assert.equal(catalog.loadingOlder, false);
    const retry = catalog.older();
    assert.equal(pending[2]?.cursor, 'older');
    pending[2]!.finish([run('retried')]);
    assert.equal((await retry).current(), true);
    assert.deepEqual(
      catalog.runs.map((row) => row.id),
      ['first', 'retried'],
    );
    assert.equal(catalog.hasOlder, false);
  } finally {
    catalog.pause();
    globalThis.fetch = original;
  }
});

test('refresh replaces pagination ownership and stale completion cannot release the new page', async () => {
  const original = globalThis.fetch;
  const pending = deferredReads();
  const catalog = new RecordingCatalog();
  try {
    const initial = catalog.refresh(() => '');
    pending[0]!.finish([run('old-first')], 'old-cursor');
    await initial;
    const oldPage = catalog.older();
    const refresh = catalog.refresh(() => '');
    assert.equal(catalog.loadingOlder, false);
    assert.equal(pending[1]?.signal.aborted, true);
    const duringRefresh = catalog.older();
    assert.equal(pending.length, 3);
    assert.equal((await duringRefresh).current(), false);
    pending[2]!.finish([run('new-first')], 'new-cursor');
    await refresh;

    const newPage = catalog.older();
    assert.equal(pending[3]?.cursor, 'new-cursor');
    pending[1]!.finish([run('stale')], 'stale-cursor');
    assert.equal((await oldPage).current(), false);
    assert.equal(catalog.loadingOlder, true);
    const anotherConsumer = catalog.older();
    assert.equal(pending.length, 4);
    pending[3]!.finish([run('new-second')]);
    assert.equal((await newPage).current(), true);
    assert.equal((await anotherConsumer).current(), true);
    assert.equal(catalog.loadingOlder, false);
    assert.deepEqual(
      catalog.runs.map((row) => row.id),
      ['new-first', 'new-second'],
    );
    assert.equal(catalog.hasOlder, false);

    catalog.pause();
    assert.equal((await catalog.older()).current(), false);
    assert.equal(pending.length, 4);
  } finally {
    catalog.pause();
    globalThis.fetch = original;
  }
});
