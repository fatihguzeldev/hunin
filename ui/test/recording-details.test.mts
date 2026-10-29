import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError } from '../src/api.js';
import {
  recordingDetails,
  recordingMetadata,
  queryFailureMessage,
} from '../src/recording-details.js';

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
