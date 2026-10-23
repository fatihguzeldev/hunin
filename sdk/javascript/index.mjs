import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { closeSync, fsyncSync, mkdirSync, openSync, writeSync } from 'node:fs';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { threadId } from 'node:worker_threads';

const ambient = new AsyncLocalStorage();
const producer = randomUUID();
let run, directory;
try { run = process.env.HUNIN_RUN_ID; directory = process.env.HUNIN_RECORD_DIR; } catch {} // Deno permission denial disables optional instrumentation.
const active = Boolean(run && directory);
let fd;
let failure;
let pending = '';
let sequence = 0;

function now() { return Math.floor(performance.timeOrigin * 1000 + performance.now() * 1000); }
function identity() { return `${producer}:${++sequence}`; }
function append(kind, fields) {
  if (!active || failure) return;
  try {
    const state = ambient.getStore() ?? {};
    const row = {
      schema_version: 1, kind, id: identity(), run_id: run, producer_id: producer,
      pid: process.pid, tid: String(threadId), timestamp_us: now(), basis: 'explicit',
      ...(state.context_id ? { context_id: state.context_id } : {}), ...fields,
      attributes: { thread_id_source: 'node:worker_threads.threadId', clock: 'performance.timeOrigin+performance.now', ...fields.attributes },
    };
    pending += JSON.stringify(row) + '\n';
    if (pending.length >= 65536) flush();
    return row.id;
  } catch (error) { failure = error; }
}
export function enabled() { return active && !failure; }
export function lastError() { return failure; }
export function flush() {
  if (!active || failure || !pending) return;
  try {
    if (fd === undefined) {
      mkdirSync(directory, { recursive: true, mode: 0o700 });
      fd = openSync(join(directory, `sdk-js-${process.pid}-${threadId}-${producer}.jsonl`), 'a', 0o600);
    }
    const bytes = Buffer.from(pending);
    let offset = 0;
    while (offset < bytes.length) offset += writeSync(fd, bytes, offset, bytes.length - offset);
    pending = '';
    fsyncSync(fd);
  } catch (error) { failure = error; }
}
export function captureContext() {
  if (!active) return undefined;
  return { run_id: run, ...ambient.getStore() };
}
export function context(name, attributes = {}) {
  return append('context', { name, attributes });
}
export function withContext(value, callback, attributes = {}) {
  if (!active) return callback();
  const token = typeof value === 'string' ? { run_id: run, context_id: context(value, attributes) } : value;
  if (!token || token.run_id !== run) return callback();
  return ambient.run({ context_id: token.context_id, parent_id: token.parent_id }, callback);
}
export function startSpan(name, attributes = {}) {
  const parent = ambient.getStore();
  const id = active ? identity() : undefined;
  const timestamp = active ? now() : 0;
  const started = active ? performance.now() : 0;
  let ended = false;
  return {
    id,
    end(extra = {}) {
      if (ended || !active) return;
      ended = true;
      append('span', {
        id, name, timestamp_us: timestamp, duration_us: Math.max(0, Math.floor((performance.now() - started) * 1000)),
        ...(parent?.parent_id ? { parent_id: parent.parent_id } : {}),
        ...(parent?.context_id ? { context_id: parent.context_id } : {}),
        attributes: { ...attributes, ...extra, clock: 'performance.timeOrigin+performance.now' },
      });
    },
    run(callback) { return ambient.run({ ...parent, parent_id: id }, callback); },
  };
}
export function withSpan(name, callback, attributes = {}) {
  if (!active) return callback();
  const span = startSpan(name, attributes);
  return span.run(() => {
    try {
      const result = callback(span);
      if (result && typeof result.then === 'function') {
        return Promise.resolve(result).then(value => { span.end(); return value; }, error => {
          span.end({ status: 'error', error: String(error) }); throw error;
        });
      }
      span.end();
      return result;
    } catch (error) { span.end({ status: 'error', error: String(error) }); throw error; }
  });
}
export function counter(name, value, unit = 'count', attributes = {}) {
  if (!Number.isFinite(value)) return;
  return append('metric', { name, value, unit, scope: ambient.getStore()?.context_id ? 'context' : 'thread', attributes: { ...attributes, temporality: 'delta' } });
}
export function event(name, attributes = {}) { return append('event', { name, attributes }); }
export function relation(from, to, name = 'links', evidence = []) {
  if (!from || !to) return;
  return append('relation', { from, to, relation: name, evidence: [...new Set(evidence)] });
}
process.on('exit', () => { flush(); if (fd !== undefined) { try { closeSync(fd); } catch {} } });
