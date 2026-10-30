import { AsyncLocalStorage } from 'node:async_hooks';
import { performance } from 'node:perf_hooks';
import { append, enabled, identity, now, recording } from './writer.mjs';

export { enabled, flush, lastError } from './writer.mjs';

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
const ambient = new AsyncLocalStorage<AmbientContext>();

function errorMessage(error: unknown): string {
  try {
    return String(error);
  } catch {
    return 'Error message unavailable';
  }
}

export function captureContext(): Context | undefined {
  if (!recording || !enabled()) return undefined;
  return { run_id: recording.runId, ...ambient.getStore() };
}

export function context(
  name: string,
  attributes: Attributes = {},
): string | undefined {
  return append('context', { name, attributes }, ambient.getStore());
}

export function withContext<T>(
  value: string | Context | undefined,
  callback: () => T,
  attributes: Attributes = {},
): T {
  if (!recording || !enabled()) return callback();
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
      if (ended || !recording || !enabled()) return;
      ended = true;
      append(
        'span',
        {
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
            clock: 'unix_epoch',
            clock_alignment: 'direct_epoch',
            clock_source: 'performance.timeOrigin+performance.now',
          },
        },
        ambient.getStore(),
      );
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
  if (!recording || !enabled()) return callback();
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
  return append(
    'metric',
    {
      name,
      value,
      unit,
      scope: ambient.getStore()?.context_id ? 'context' : 'thread',
      attributes: { ...attributes, temporality: 'delta' },
    },
    ambient.getStore(),
  );
}

export function event(
  name: string,
  attributes: Attributes = {},
): string | undefined {
  return append('event', { name, attributes }, ambient.getStore());
}

export function relation(
  from: string | undefined,
  to: string | undefined,
  name = 'links',
  evidence: string[] = [],
): string | undefined {
  if (!from || !to) return;
  return append(
    'relation',
    {
      from,
      to,
      relation: name,
      evidence: [...new Set(evidence)],
    },
    ambient.getStore(),
  );
}
