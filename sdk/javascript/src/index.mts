import { AsyncLocalStorage } from 'node:async_hooks';
import { randomUUID } from 'node:crypto';
import { closeSync, fsyncSync, mkdirSync, openSync, writeSync } from 'node:fs';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { threadId } from 'node:worker_threads';

export type Attributes = Record<string, unknown>;

export interface Context {
  run_id: string;
  context_id?: string;
  parent_id?: string;
}

export interface Span {
  readonly id: string | undefined;
  end(attributes?: Attributes): void;
  run<T>(callback: () => T): T;
}

type AmbientContext = Omit<Context, 'run_id'>;
type Fields = {
  context: { name: string; attributes: Attributes };
  span: {
    id: string | undefined;
    name: string;
    timestamp_us: number;
    duration_us: number;
    parent_id?: string;
    context_id?: string;
    attributes: Attributes;
  };
  metric: {
    name: string;
    value: number;
    unit: string;
    scope: 'context' | 'thread';
    attributes: Attributes;
  };
  event: { name: string; attributes: Attributes };
  relation: { from: string; to: string; relation: string; evidence: string[] };
};

function configuredRecording():
  | { runId: string; directory: string }
  | undefined {
  try {
    const runId = process.env.HUNIN_RUN_ID;
    const directory = process.env.HUNIN_RECORD_DIR;
    return runId && directory ? { runId, directory } : undefined;
  } catch {
    // Deno permission denial disables optional instrumentation.
    return undefined;
  }
}

const ambient = new AsyncLocalStorage<AmbientContext>();
const producer = randomUUID();
const recording = configuredRecording();
let fd: number | undefined;
let failure: unknown;
let pending = '';
let sequence = 0;

function now(): number {
  return Math.floor(performance.timeOrigin * 1000 + performance.now() * 1000);
}

function identity(): string {
  return `${producer}:${++sequence}`;
}

function fail(error: unknown): void {
  failure =
    error instanceof Error
      ? error
      : new Error('Hunin observation serialization or persistence failed', {
          cause: error,
        });
}

function errorMessage(error: unknown): string {
  try {
    return String(error);
  } catch {
    return 'Error message unavailable';
  }
}

function append<K extends keyof Fields>(
  kind: K,
  fields: Fields[K],
): string | undefined {
  if (!recording || failure) return;
  try {
    const state = ambient.getStore() ?? {};
    const row = {
      schema_version: 1,
      kind,
      id: identity(),
      run_id: recording.runId,
      producer_id: producer,
      pid: process.pid,
      tid: String(threadId),
      timestamp_us: now(),
      basis: 'explicit',
      ...(state.context_id ? { context_id: state.context_id } : {}),
      ...fields,
      attributes: {
        thread_id_source: 'node:worker_threads.threadId',
        clock: 'performance.timeOrigin+performance.now',
        ...('attributes' in fields ? fields.attributes : {}),
      },
    };
    pending += JSON.stringify(row) + '\n';
    if (pending.length >= 65536) flush();
    return failure ? undefined : row.id;
  } catch (error) {
    fail(error);
  }
}

export function enabled(): boolean {
  return Boolean(recording && !failure);
}

export function lastError(): unknown {
  return failure;
}

export function flush(): void {
  if (!recording || failure || !pending) return;
  try {
    if (fd === undefined) {
      mkdirSync(recording.directory, { recursive: true, mode: 0o700 });
      fd = openSync(
        join(
          recording.directory,
          `sdk-js-${process.pid}-${threadId}-${producer}.jsonl`,
        ),
        'a',
        0o600,
      );
    }
    const bytes = Buffer.from(pending);
    let offset = 0;
    while (offset < bytes.length) {
      offset += writeSync(fd, bytes, offset, bytes.length - offset);
    }
    pending = '';
    fsyncSync(fd);
  } catch (error) {
    fail(error);
  }
}

export function captureContext(): Context | undefined {
  if (!recording || failure) return undefined;
  return { run_id: recording.runId, ...ambient.getStore() };
}

export function context(
  name: string,
  attributes: Attributes = {},
): string | undefined {
  return append('context', { name, attributes });
}

export function withContext<T>(
  value: string | Context | undefined,
  callback: () => T,
  attributes: Attributes = {},
): T {
  if (!recording || failure) return callback();
  const token =
    typeof value === 'string'
      ? { run_id: recording.runId, context_id: context(value, attributes) }
      : value;
  if (!token || token.run_id !== recording.runId) return callback();
  return ambient.run(
    { context_id: token.context_id, parent_id: token.parent_id },
    callback,
  );
}

export function startSpan(name: string, attributes: Attributes = {}): Span {
  const parent = ambient.getStore();
  const active = enabled();
  const id = active ? identity() : undefined;
  const timestamp = active ? now() : 0;
  const started = active ? performance.now() : 0;
  let ended = false;
  return {
    id,
    end(extra = {}) {
      if (ended || !recording || failure) return;
      ended = true;
      append('span', {
        id,
        name,
        timestamp_us: timestamp,
        duration_us: Math.max(
          0,
          Math.floor((performance.now() - started) * 1000),
        ),
        ...(parent?.parent_id ? { parent_id: parent.parent_id } : {}),
        ...(parent?.context_id ? { context_id: parent.context_id } : {}),
        attributes: {
          ...attributes,
          ...extra,
          clock: 'performance.timeOrigin+performance.now',
        },
      });
    },
    run(callback) {
      return ambient.run({ ...parent, parent_id: id }, callback);
    },
  };
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
  return (
    Boolean(value) && typeof (value as { then?: unknown }).then === 'function'
  );
}

export function withSpan<T>(
  name: string,
  callback: (span?: Span) => T,
  attributes?: Attributes,
): T;
export function withSpan(
  name: string,
  callback: (span?: Span) => unknown,
  attributes: Attributes = {},
): unknown {
  if (!recording || failure) return callback();
  const span = startSpan(name, attributes);
  return span.run(() => {
    try {
      const result = callback(span);
      if (isThenable(result)) {
        return Promise.resolve(result).then(
          (value) => {
            span.end();
            return value;
          },
          (error: unknown) => {
            span.end({ status: 'error', error: errorMessage(error) });
            throw error;
          },
        );
      }
      span.end();
      return result;
    } catch (error) {
      span.end({ status: 'error', error: errorMessage(error) });
      throw error;
    }
  });
}

export function counter(
  name: string,
  value: number,
  unit = 'count',
  attributes: Attributes = {},
): string | undefined {
  if (!Number.isFinite(value)) return;
  return append('metric', {
    name,
    value,
    unit,
    scope: ambient.getStore()?.context_id ? 'context' : 'thread',
    attributes: { ...attributes, temporality: 'delta' },
  });
}

export function event(
  name: string,
  attributes: Attributes = {},
): string | undefined {
  return append('event', { name, attributes });
}

export function relation(
  from: string | undefined,
  to: string | undefined,
  name = 'links',
  evidence: string[] = [],
): string | undefined {
  if (!from || !to) return;
  return append('relation', {
    from,
    to,
    relation: name,
    evidence: [...new Set(evidence)],
  });
}

process.on('exit', () => {
  flush();
  if (fd !== undefined) {
    try {
      closeSync(fd);
    } catch {
      // Process exit cannot recover a descriptor-close error.
    }
  }
});
