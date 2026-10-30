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

test('payload-free page locations restore opaque cursors and previous first-page navigation', async () => {
  const read = async (cursor: string | undefined): Promise<Page<string>> => ({
    items: [cursor ?? 'first'],
    next_cursor: cursor === undefined ? 'opaque-next' : undefined,
  });
  const query = new PagedQuery(read);
  await query.refresh();
  await query.next();
  assert.deepEqual(query.location, { cursor: 'opaque-next', previous: [null] });
  const location = JSON.parse(JSON.stringify(query.location));
  const restored = new PagedQuery(read, new AbortController().signal, location);
  await restored.refresh();
  assert.deepEqual(restored.page.items, ['opaque-next']);
  assert.equal(restored.hasPrevious, true);
  await restored.previous();
  assert.deepEqual(restored.page.items, ['first']);
  assert.deepEqual(restored.location, { cursor: undefined, previous: [] });
});

test('metadata page navigation retains history, handles failures, and rejects stale refreshes', async () => {
  const requested: (string | undefined)[] = [];
  let fail = false;
  const pages = new PagedQuery(async (cursor) => {
    requested.push(cursor);
    if (fail) throw new Error('unavailable');
    return {
      items: [cursor ?? 'first'],
      next_cursor: cursor ? null : 'opaque-next',
    };
  });
  await pages.refresh();
  await pages.next();
  assert.deepEqual(pages.page.items, ['opaque-next']);
  assert.equal(pages.hasPrevious, true);
  await pages.refresh(); // Polling keeps the historical page selected.
  assert.equal(requested.at(-1), 'opaque-next');
  fail = true;
  await assert.rejects(pages.previous(), /unavailable/);
  assert.deepEqual(pages.page.items, ['opaque-next']);
  assert.equal(pages.hasPrevious, true);
  fail = false;
  await pages.previous();
  assert.deepEqual(pages.page.items, ['first']);
  assert.equal(pages.hasPrevious, false);
  await pages.next();
  await pages.refresh(true);
  assert.deepEqual(pages.page.items, ['first']);
  assert.equal(pages.hasPrevious, false);

  const pending: {
    resolve: (page: Page<string>) => void;
    reject: (error: Error) => void;
  }[] = [];
  const concurrent = new PagedQuery<string>(
    () =>
      new Promise<Page<string>>((resolve, reject) =>
        pending.push({ resolve, reject }),
      ),
  );
  const old = concurrent.refresh();
  const current = concurrent.refresh(true);
  pending[1].resolve({ items: ['current'] });
  assert.equal(await current, true);
  pending[0].resolve({ items: ['stale'] });
  assert.equal(await old, false);
  assert.deepEqual(concurrent.page.items, ['current']);
  const staleFailure = concurrent.refresh();
  const latest = concurrent.refresh();
  pending[3].resolve({ items: ['latest'] });
  await latest;
  pending[2].reject(new Error('stale failure'));
  assert.equal(await staleFailure, false);
  assert.deepEqual(concurrent.page.items, ['latest']);
});
