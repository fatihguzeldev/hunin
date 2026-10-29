import assert from 'node:assert/strict';
import process from 'node:process';
import { createInterface } from 'node:readline/promises';
import * as h from '../javascript/dist/index.mjs';

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function transferredContext(text: string): h.Context | undefined {
  const token: unknown = JSON.parse(text);
  if (token === null) return undefined;
  assert.ok(isObject(token));
  // Disabled producers historically transfer captureContext() ?? {}.
  if (Object.keys(token).length === 0) return undefined;
  assert.ok(typeof token.run_id === 'string');
  const result: h.Context = { run_id: token.run_id };
  for (const key of ['context_id', 'parent_id'] as const) {
    if (key in token) {
      const value: unknown = token[key];
      assert.ok(typeof value === 'string');
      result[key] = value;
    }
  }
  return result;
}

const child = process.argv.includes('--child');
if (child) {
  const token = transferredContext(process.env.HUNIN_SDK_TOKEN || 'null');
  h.withContext(token, () =>
    h.withSpan('remote', (span) => {
      h.counter('jobs', 1);
      console.log('HUNIN_REMOTE ' + (span?.id ?? ''));
    }),
  );
} else {
  await h.withContext('request', () =>
    h.withSpan('root', async (root) => {
      await Promise.all(
        ['a', 'b'].map((name) =>
          h.withSpan(name, async () => {
            await new Promise((resolve) => setTimeout(resolve, 1));
            h.withSpan(name + '-child', () => h.counter('jobs', 1));
          }),
        ),
      );
      console.log('HUNIN_TOKEN ' + JSON.stringify(h.captureContext() ?? {}));
      const input = createInterface({ input: process.stdin });
      const remote = await input.question('');
      input.close();
      if (root?.id && remote) h.relation(root.id, remote, 'dispatches');
    }),
  );
}
h.flush();
if (h.lastError()) throw h.lastError();
