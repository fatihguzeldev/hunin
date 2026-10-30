import test from 'node:test';
import assert from 'node:assert/strict';
import {
  functionSelection,
  countLabel,
  countStatus,
} from '../src/call-counts.js';

test('selection keeps exact source disambiguation and rejects a missing function name', () => {
  assert.deepEqual(functionSelection(' work ', 'src/main.ts', '12'), {
    name: 'work',
    file: 'src/main.ts',
    line: 12,
  });
  assert.deepEqual(functionSelection('work', '', ''), { name: 'work' });
  assert.throws(
    () => functionSelection('', 'src/main.ts', ''),
    /function name/,
  );
  assert.throws(() => functionSelection('work', '', '0'), /positive/);
  assert.throws(() => functionSelection('work', '', '1.5'), /positive/);
});

test('missing observations remain missing while an observed zero stays zero', () => {
  assert.equal(countLabel(undefined), 'Not observed');
  assert.equal(countLabel(null), 'Not observed');
  assert.equal(countLabel(0), '0');
  assert.equal(countStatus('not_collected'), 'Not collected');
  assert.equal(countStatus('incomplete'), 'Incomplete');
});

test('counting selections and the separate measurement handle survive private navigation history', async () => {
  const { Investigations } = await import('../src/investigation/state.js');
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
  const first = new Investigations(storage);
  first.forRun('sampling-parent').calls = {
    functions: [{ name: 'work', file: 'a.ts', line: 2 }],
    measurementRunId: 'counting-child',
  };
  first.persist('sampling-parent');
  assert.deepEqual(
    new Investigations(storage).forRun('sampling-parent').calls,
    first.forRun('sampling-parent').calls,
  );
  assert.deepEqual(new Investigations(storage).forRun('other-run').calls, {
    functions: [],
  });
});

test('uncertain measurement retries reuse their key, while a new accepted measurement gets a fresh key', async () => {
  const { CountAttempt } = await import('../src/call-counts.js');
  let serial = 0;
  const attempt = new CountAttempt(() => `request-${++serial}`);
  const params = { run_id: 'parent', functions: [{ name: 'work' }] };
  assert.equal(attempt.request(params).request_id, 'request-1');
  assert.equal(attempt.request({ ...params }).request_id, 'request-1');
  assert.equal(
    attempt.request({ ...params, functions: [] }).request_id,
    'request-2',
  );
  attempt.accepted();
  assert.equal(attempt.request(params).request_id, 'request-3');
});

test('paging cannot combine different counting recordings or source scopes', async () => {
  const { appendCountPage } = await import('../src/call-counts.js');
  const page = {
    status: 'complete' as const,
    run_id: 'child',
    parent_run_id: 'parent',
    method: 'counter',
    requested_scope: 'all' as const,
    collection_scope: 'all' as const,
    counts: [
      {
        function: { name: 'work', file: null, line: null },
        calls: 0,
        resumes: null,
      },
    ],
    next_cursor: null,
    total_count: 2,
    diagnostics: [],
    limitations: [],
  };
  assert.equal(appendCountPage(page, page).counts.length, 2);
  assert.throws(
    () => appendCountPage(page, { ...page, run_id: 'another-child' }),
    /recording or scope changed/,
  );
  assert.throws(
    () => appendCountPage(page, { ...page, collection_scope: 'selected' }),
    /recording or scope changed/,
  );
});

test('typed call transports retain not-collected and measured zeros without starting another workload', async () => {
  const { operation } = await import('../src/api.js');
  const original = globalThis.fetch;
  const requests: string[] = [];
  const report = {
    status: 'not_collected',
    run_id: 'parent',
    parent_run_id: null,
    method: null,
    requested_scope: 'all',
    collection_scope: 'all',
    counts: [],
    next_cursor: null,
    total_count: 0,
    diagnostics: [],
    limitations: [],
  };
  globalThis.fetch = async (_input, init) => {
    requests.push(JSON.parse(String(init?.body)).operation);
    return Response.json({ result: report });
  };
  try {
    assert.equal(
      (await operation('calls.query', { run_id: 'parent' })).status,
      'not_collected',
    );
    Object.assign(report, {
      status: 'incomplete',
      run_id: 'child',
      parent_run_id: 'parent',
      method: 'counter',
      counts: [
        {
          function: { name: 'work', file: null, line: null },
          calls: 0,
          resumes: null,
        },
      ],
      total_count: 1,
    });
    const measured = await operation('calls.query', { run_id: 'child' });
    assert.equal(measured.counts[0]?.calls, 0);
    assert.equal(measured.counts[0]?.resumes, null);
    assert.deepEqual(requests, ['calls.query', 'calls.query']);
  } finally {
    globalThis.fetch = original;
  }
});

test('an old completed read cannot unlock a pending or newly admitted measurement', async () => {
  const { CountActivity } = await import('../src/call-counts.js');
  const activity = new CountActivity();
  let accept!: (id: string) => void;
  const admission = new Promise<string>((resolve) => {
    accept = resolve;
  });
  assert.equal(activity.begin(), true);
  const accepted = admission.then((id) => activity.accepted(id));
  activity.observed('old-child', 'complete');
  assert.equal(activity.busy, true);
  assert.equal(activity.begin(), false);
  accept('new-child');
  await accepted;
  activity.observed('old-child', 'complete');
  assert.equal(activity.runId, 'new-child');
  assert.equal(activity.busy, true);
  assert.equal(activity.begin(), false);
  activity.observed('new-child', 'running');
  assert.equal(activity.busy, true);
  activity.observed('new-child', 'incomplete');
  assert.equal(activity.busy, false);
  assert.equal(activity.begin(), true);
});

test('rejected acceptance releases activity without discarding the uncertain retry identity', async () => {
  const { CountActivity } = await import('../src/call-counts.js');
  const activity = new CountActivity();
  const attempt = new (await import('../src/call-counts.js')).CountAttempt(
    () => 'same-request',
  );
  const params = { run_id: 'parent' };
  assert.equal(activity.begin(), true);
  const first = attempt.request(params);
  activity.rejected();
  assert.equal(activity.busy, false);
  assert.equal(activity.begin(), true);
  assert.equal(attempt.request(params).request_id, first.request_id);
});
