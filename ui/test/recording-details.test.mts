import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError, operation } from '../src/api.js';
import {
  runSource,
  runCorrectness,
  runStatusTone,
} from '../src/recordings/presentation.js';
import {
  recordingDetails,
  recordingMetadata,
  queryFailureMessage,
} from '../src/recordings/details.js';

const run = {
  id: 'large-recording',
  name: 'Synthetic scale control',
  runtime: 'imported',
  command: [],
  cwd: '/fixture',
  started_at_us: 1,
  finished_at_us: 2,
  status: 'completed',
  source: { fingerprint: 'source' },
  correctness: null,
  exit_code: 0,
  diagnostics: [],
};
const summary = {
  sample_count: 0,
  span_count: 0,
  relation_count: 0,
  metric_count: 0,
  total_weight: 0,
  weight_unit: 'samples',
  diagnostics: [],
};

function budgetFailure(): Response {
  return Response.json(
    {
      error: {
        code: 'operation_failed',
        message: 'summary exceeded its retained payload budget',
        details: {
          operation_error: {
            code: 'work_budget_exceeded',
            operation: 'runs.get',
            message: 'summary exceeded its retained payload budget',
            budget: {
              scope: 'run-summary',
              dimension: 'bytes',
              limit: 10,
              observed: 11,
            },
          },
        },
      },
    },
    { status: 500 },
  );
}

test('a checked summary preserves measured zero and its result identity', async () => {
  const original = globalThis.fetch;
  const calls: string[] = [];
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body));
    calls.push(request.operation);
    return Response.json({
      result: { run, summary, result_identity: 'selected-a' },
    });
  };
  try {
    const data = await recordingDetails(run.id, new AbortController().signal);
    assert.deepEqual(data.summary, summary);
    assert.equal(data.result_identity, 'selected-a');
    assert.deepEqual(calls, ['runs.get']);
  } finally {
    globalThis.fetch = original;
  }
});

test('a typed aggregate budget failure retains the run and optional metadata identity without fabricating a summary', async () => {
  const original = globalThis.fetch;
  const calls: string[] = [];
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body));
    calls.push(request.operation);
    return request.operation === 'runs.get'
      ? budgetFailure()
      : Response.json({
          result: {
            run,
            result_identity_diagnostic: 'selection ID budget unavailable',
          },
        });
  };
  try {
    const data = await recordingDetails(run.id, new AbortController().signal);
    assert.equal(data.run.id, run.id);
    assert.equal(data.summary, undefined);
    assert.equal(data.result_identity, undefined);
    assert.equal(
      data.result_identity_diagnostic,
      'selection ID budget unavailable',
    );
    assert.ok(data.summaryError instanceof ApiError);
    assert.match(
      queryFailureMessage(data.summaryError),
      /^work_budget_exceeded · summary exceeded/,
    );
    assert.deepEqual(calls, ['runs.get', 'runs.metadata']);
  } finally {
    globalThis.fetch = original;
  }
});

test('disposed reads never start a metadata fallback even if a failed transport ignored its signal', async () => {
  const original = globalThis.fetch;
  const controller = new AbortController();
  const calls: string[] = [];
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body));
    calls.push(request.operation);
    controller.abort();
    return budgetFailure();
  };
  try {
    await assert.rejects(recordingDetails(run.id, controller.signal), {
      name: 'AbortError',
    });
    assert.deepEqual(calls, ['runs.get']);
  } finally {
    globalThis.fetch = original;
  }
});

test('failed metadata fallback preserves the original aggregate error for a service without the additive operation', async () => {
  const original = globalThis.fetch;
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body));
    return request.operation === 'runs.get'
      ? budgetFailure()
      : Response.json(
          {
            error: {
              code: 'unknown_operation',
              message: 'unknown runs.metadata',
            },
          },
          { status: 400 },
        );
  };
  try {
    await assert.rejects(
      recordingDetails(run.id, new AbortController().signal),
      /summary exceeded its retained payload budget/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('status polling reads metadata without invoking an aggregate query', async () => {
  const original = globalThis.fetch;
  const calls: string[] = [];
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body));
    calls.push(request.operation);
    return Response.json({
      result: {
        run: { ...run, status: 'running' },
        result_identity: 'selected-b',
      },
    });
  };
  try {
    const data = await recordingMetadata(run.id, new AbortController().signal);
    assert.equal(data.run.status, 'running');
    assert.equal(data.result_identity, 'selected-b');
    assert.deepEqual(calls, ['runs.metadata']);
  } finally {
    globalThis.fetch = original;
  }
});

test('legacy status polling falls back only for the machine unknown-operation code', async () => {
  const original = globalThis.fetch;
  const calls: string[] = [];
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body));
    calls.push(request.operation);
    return request.operation === 'runs.metadata'
      ? Response.json(
          { error: { code: 'unknown_operation', message: 'legacy service' } },
          { status: 400 },
        )
      : Response.json({
          result: { run, summary, result_identity: 'legacy-selection' },
        });
  };
  try {
    const data = await recordingMetadata(run.id, new AbortController().signal);
    assert.equal(data.result_identity, 'legacy-selection');
    assert.equal('summary' in data, false);
    assert.deepEqual(calls, ['runs.metadata', 'runs.get']);
    calls.length = 0;
    globalThis.fetch = async (_input, init) => {
      calls.push(JSON.parse(String(init?.body)).operation);
      throw new Error('network unavailable, not unknown_operation');
    };
    await assert.rejects(
      recordingMetadata(run.id, new AbortController().signal),
      /network unavailable/,
    );
    assert.deepEqual(calls, ['runs.metadata']);
  } finally {
    globalThis.fetch = original;
  }
});

test('artifact responses preserve null cursors and byte offsets while rejecting malformed results', async () => {
  const original = globalThis.fetch;
  let result: unknown;
  try {
    globalThis.fetch = async () => Response.json({ result });
    result = {
      artifacts: [
        { id: 'a1', name: 'profile', size: 4, sha256: 'digest', format: 'raw' },
      ],
      next_cursor: null,
    };
    assert.deepEqual(
      await operation('artifacts.list', { run_id: 'r1' }),
      result,
    );
    result = {
      content: '\ufffd',
      encoding: 'utf8_lossy',
      next_offset: 3,
      size: 4,
    };
    assert.deepEqual(
      await operation('artifacts.read', {
        run_id: 'r1',
        artifact_id: 'a1',
        offset: 2,
        limit: 1,
      }),
      result,
    );
    result = {
      content: '',
      encoding: 'utf8_lossy',
      next_offset: null,
      size: 4,
      future_metadata: true,
    };
    assert.deepEqual(
      await operation('artifacts.read', {
        run_id: 'r1',
        artifact_id: 'a1',
        offset: 4,
      }),
      result,
    );
    for (const malformed of [
      { artifacts: [], next_cursor: 4 },
      { artifacts: [] },
      {
        artifacts: [
          {
            id: 'a1',
            name: 'profile',
            size: '4',
            sha256: 'digest',
            format: 'raw',
          },
        ],
        next_cursor: null,
      },
    ]) {
      result = malformed;
      await assert.rejects(
        operation('artifacts.list', { run_id: 'r1' }),
        /invalid artifacts.list result/,
      );
    }
    result = { content: '', encoding: 'base64', next_offset: null, size: 4 };
    await assert.rejects(
      operation('artifacts.read', { run_id: 'r1', artifact_id: 'a1' }),
      /invalid artifacts.read result/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('run responses retain legacy metadata and extension statuses but reject malformed fixed fields', async () => {
  const original = globalThis.fetch;
  const run = {
    id: 'legacy',
    name: 'legacy',
    status: 'archived-by-extension',
    command: [],
    cwd: '/project',
    runtime: 'imported',
    started_at_us: 1,
    finished_at_us: null,
    exit_code: null,
    diagnostics: [],
    source: null,
    correctness: 'unverified',
  };
  let result: unknown = { runs: [run], next_cursor: null };
  try {
    globalThis.fetch = async () => Response.json({ result });
    assert.deepEqual(await operation('runs.list', {}), result);
    result = {
      runs: [{ ...run, finished_at_us: 'yesterday' }],
      next_cursor: null,
    };
    await assert.rejects(
      operation('runs.list', {}),
      /invalid runs.list result/,
    );
    const details = {
      run,
      summary: {
        sample_count: 0,
        total_weight: 0,
        weight_unit: 'samples',
        span_count: 0,
        metric_count: 0,
        relation_count: 0,
        diagnostics: [],
      },
    };
    result = details;
    assert.deepEqual(
      await operation('runs.get', { run_id: 'legacy' }),
      details,
    );
    result = { ...details, result_identity: 'opaque-snapshot' };
    assert.deepEqual(await operation('runs.get', { run_id: 'legacy' }), result);
    result = { ...details, result_identity: 7 };
    await assert.rejects(
      operation('runs.get', { run_id: 'legacy' }),
      /invalid runs.get result/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('run presentation reads known metadata safely without changing retained values', () => {
  for (const value of [
    null,
    'legacy',
    42,
    [],
    { fingerprint: 7, revision: false, passed: 'yes' },
  ]) {
    const run = { source: value, correctness: value };
    assert.equal(runSource(run).fingerprint, 'unknown');
    assert.equal(runCorrectness(run).passed, null);
    assert.equal(run.source, value);
    assert.equal(run.correctness, value);
  }
  assert.deepEqual(
    runSource({
      source: {
        fingerprint: 'measured',
        revision: null,
        dirty: true,
        build_source_verified: false,
      },
    }),
    {
      fingerprint: 'measured',
      revision: undefined,
      dirty: true,
      executable_sha256: undefined,
      runtime_version: undefined,
      build_source_verified: false,
    },
  );
  assert.equal(
    runCorrectness({
      correctness: { passed: false, reason: 'mismatch', actual_exit_code: 1 },
    }).passed,
    false,
  );
  assert.equal(runStatusTone('completed'), 'good');
  assert.equal(runStatusTone('archived-by-extension'), '');
  for (const status of ['constructor', '__proto__', 'toString']) {
    assert.equal(runStatusTone(status), '');
  }
});
