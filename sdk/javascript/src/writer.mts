/** Process-owned producer identity, buffered JSONL output and sticky failures. */
import { randomUUID } from 'node:crypto';
import { closeSync, fsyncSync, mkdirSync, openSync, writeSync } from 'node:fs';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { threadId } from 'node:worker_threads';
import type { Attributes, Context } from './index.mjs';

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

const producer = randomUUID();
export const recording = configuredRecording();
let fd: number | undefined;
let failure: unknown;
let pending = '';
let sequence = 0;

export function now(): number {
  return Math.floor(performance.timeOrigin * 1000 + performance.now() * 1000);
}

export function identity(): string {
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

export function append<K extends keyof Fields>(
  kind: K,
  fields: Fields[K],
  state: AmbientContext = {},
): string | undefined {
  if (!recording || failure) return;
  try {
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
        clock: 'unix_epoch',
        clock_alignment: 'direct_epoch',
        clock_source: 'performance.timeOrigin+performance.now',
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
