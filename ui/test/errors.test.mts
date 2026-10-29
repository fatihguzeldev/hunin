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
