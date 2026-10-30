import test from 'node:test';
import assert from 'node:assert/strict';
import { typeOf } from '../scripts/generate-record.mjs';

test('named properties preserve open schema extensions without opening closed objects', () => {
  const open = typeOf({
    type: 'object',
    properties: { clock_alignment: { type: 'string' } },
  });
  assert.ok(open.includes('"clock_alignment"?: string;'));
  assert.ok(open.includes('[key: string]: unknown;'));
  const closed = typeOf({
    type: 'object',
    properties: { clock_alignment: { type: 'string' } },
    additionalProperties: false,
  });
  assert.ok(!closed.includes('[key: string]'));
});
