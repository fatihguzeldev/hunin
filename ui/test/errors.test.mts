import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError, operation } from '../src/api.js';
import {
  isHttpFailure,
  isMcpToolFailure,
  isOperationFailure,
} from '../src/operations.generated.js';

test('typed-origin errors share a checked machine contract while legacy details remain arbitrary JSON', async () => {
  const failure = {
    code: 'invalid_parameters',
    operation: 'runs.start',
    message: 'Command is required',
  };
  const envelope = {
    error: {
      code: 'operation_failed',
      message: 'Legacy message',
      details: { operation_error: failure },
    },
  };
  assert.equal(isOperationFailure(failure), true);
  assert.equal(
    isOperationFailure({ ...failure, code: 'guessed_from_message' }),
    false,
  );
  assert.equal(
    isOperationFailure({
      ...failure,
      params: { secret: 'not part of this contract' },
    }),
    false,
  );
  assert.equal(isHttpFailure(envelope), true);
  assert.equal(
    isHttpFailure({
      error: {
        ...envelope.error,
        details: { operation_error: { ...failure, operation: 1 } },
      },
    }),
    false,
  );
  for (const details of [
    null,
    'legacy',
    7,
    ['retained'],
    { vendor: 'extension' },
  ])
    assert.equal(
      isHttpFailure({
        error: { code: 'legacy', message: 'retained', details },
      }),
      true,
    );
  const original = globalThis.fetch;
  try {
    globalThis.fetch = async () => Response.json(envelope, { status: 400 });
    await assert.rejects(
      operation('runs.start', { command: [] }),
      (error: unknown) => {
        assert.ok(error instanceof ApiError);
        assert.equal(error.code, 'operation_failed');
        assert.equal(error.message, 'Legacy message');
        assert.deepEqual(error.operationFailure, failure);
        return true;
      },
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('MCP machine errors remain metadata alongside the existing error text envelope', () => {
  const legacy = {
    content: [{ type: 'text', text: 'Unknown operation' }],
    isError: true,
  };
  assert.equal(isMcpToolFailure(legacy), true);
  const response = {
    ...legacy,
    _meta: {
      'hunin/operation-error': {
        code: 'unknown_operation',
        operation: 'extension.not-found',
        message: 'Unknown operation',
      },
    },
  };
  assert.equal(isMcpToolFailure(response), true);
  assert.equal(
    isMcpToolFailure({
      ...response,
      _meta: {
        'hunin/operation-error': {
          code: 'conflict',
          message: 'missing operation',
        },
      },
    }),
    false,
  );
});

test('transport preserves operation arguments and rejects backend errors', async () => {
  let captured: ({ url: RequestInfo | URL } & RequestInit) | undefined;
  const original = globalThis.fetch;
  try {
    globalThis.fetch = async (url, request) => {
      captured = { url, ...request };
      return Response.json({
        result: {
          records: [],
          unaligned_records: [],
          alignment: 'aligned',
          next_cursor: null,
          bounds: { start_us: 1, end_us: 2 },
          unaligned_count: 0,
          unaligned_clock_domains: [],
          total_count: 0,
          returned_count: 0,
        },
      });
    };
    await operation('timeline.query', {
      run_id: 'r1',
      start_us: 123,
      kind: 'sample',
      pid: 1,
    });
    assert.ok(captured);
    assert.ok(typeof captured.body === 'string');
    assert.equal(captured.url, '/api/operations');
    assert.deepEqual(JSON.parse(captured.body), {
      operation: 'timeline.query',
      params: { run_id: 'r1', start_us: 123, kind: 'sample', pid: 1 },
    });
    assert.equal(captured.credentials, 'same-origin');
    globalThis.fetch = async () =>
      Response.json(
        {
          error: {
            code: 'unsupported',
            message: 'Missing collector',
            details: { runtime: 'native' },
          },
        },
        { status: 400 },
      );
    await assert.rejects(
      operation('runs.start', { command: ['workload'] }),
      (error) =>
        error instanceof ApiError &&
        error.code === 'unsupported' &&
        error.message === 'Missing collector',
    );
    globalThis.fetch = async () => Response.json({ wrong: true });
    await assert.rejects(operation('runs.list', {}), /HTTP 200/);
    globalThis.fetch = async () => Response.json({ error: null });
    await assert.rejects(operation('runs.list', {}), /invalid error envelope/);
  } finally {
    globalThis.fetch = original;
  }
});

test('transport rejects malformed error fields instead of manufacturing an ApiError', async () => {
  const original = globalThis.fetch;
  try {
    for (const error of [
      { code: 7, message: 'failure' },
      { code: 'failure', message: [] },
      {},
    ]) {
      globalThis.fetch = async () => Response.json({ error }, { status: 400 });
      await assert.rejects(
        operation('runs.list', {}),
        /invalid error envelope/,
      );
    }
  } finally {
    globalThis.fetch = original;
  }
});
