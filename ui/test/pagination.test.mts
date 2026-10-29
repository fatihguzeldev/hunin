import test from 'node:test';
import assert from 'node:assert/strict';
import { PagedQuery, type Page } from '../src/pagination.js';

test('disposing a page query ignores late data and does not issue new reads', async () => {
  const lifetime = new AbortController();
  let finish: ((page: Page<string>) => void) | undefined;
  let reads = 0;
  const query = new PagedQuery<string>(() => {
    reads++;
    return new Promise((resolve) => {
      finish = resolve;
    });
  }, lifetime.signal);
  const pending = query.refresh();
  lifetime.abort();
  assert.ok(finish);
  finish({ items: ['stale'], next_cursor: 'old' });
  assert.equal(await pending, false);
  assert.deepEqual(query.page, { items: [] });
  assert.equal(query.hasPrevious, false);
  assert.equal(await query.refresh(), false);
  assert.equal(reads, 1);
});

test('a disposed query does not surface a late read failure as an active view error', async () => {
  const lifetime = new AbortController();
  let fail: ((reason: Error) => void) | undefined;
  const query = new PagedQuery<string>(
    () =>
      new Promise((_resolve, reject) => {
        fail = reject;
      }),
    lifetime.signal,
  );
  const pending = query.refresh();
  lifetime.abort();
  assert.ok(fail);
  fail(new Error('request was disposed'));
  assert.equal(await pending, false);
});

test('replacing an in-flight page aborts only the obsolete read and rejects late data', async () => {
  const pending: {
    signal: AbortSignal;
    finish: (page: Page<string>) => void;
  }[] = [];
  const query = new PagedQuery<string>(
    (_cursor, signal) =>
      new Promise((finish) => {
        pending.push({ signal, finish });
      }),
  );
  const obsolete = query.refresh();
  const current = query.refresh();
  assert.equal(pending[0]?.signal.aborted, true);
  assert.equal(pending[1]?.signal.aborted, false);
  pending[1]?.finish({ items: ['current'] });
  assert.equal(await current, true);
  pending[0]?.finish({ items: ['obsolete'] });
  assert.equal(await obsolete, false);
  assert.deepEqual(query.page.items, ['current']);
});
