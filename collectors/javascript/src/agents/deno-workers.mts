import inspector from 'node:inspector';
import process from 'node:process';
import {
  appendFileSync,
  readdirSync,
  openSync,
  fstatSync,
  readSync,
  closeSync,
} from 'node:fs';
import { createHash, randomUUID } from 'node:crypto';
import { join } from 'node:path';

const MAX_PROFILE_BYTES = 64 * 1024 * 1024;
const DEFAULT_TIMEOUT_MS = 2000;

interface WorkerRegistration {
  registration_id: string;
  worker_id: number | null;
  settled: boolean;
  error?: string;
  pending_error?: string;
}
interface WorkerProfile {
  worker_id: number;
  filename: string;
  sha256: string;
}
interface WorkerBarrierOptions {
  directory: string;
  runId?: string;
  producerId?: string;
  timeoutMs?: number;
}
type InspectorProperties = inspector.Runtime.GetPropertiesReturnType & {
  privateProperties?: Array<{ name: string; value?: { value?: unknown } }>;
};

// Deno closes the worker's parent channel before flushing its CPU profile and
// drops the worker JoinHandle. Keep the main event loop alive until each exact
// observed worker profile is complete. This never changes Worker.terminate().
export function installDenoWorkerBarrier({
  directory,
  runId,
  producerId,
  timeoutMs = DEFAULT_TIMEOUT_MS,
}: WorkerBarrierOptions) {
  const identity = {
    schema_version: 1,
    runtime: 'deno',
    run_id: runId,
    producer_id: producerId,
    pid: process.pid,
  };
  const path = join(directory, `deno-workers-${producerId}.jsonl`);
  const registrations: WorkerRegistration[] = [];
  const diagnostics = new Set<string>();
  const profiles = new Map<number, WorkerProfile>();
  const append = (record: Record<string, unknown>) => {
    try {
      appendFileSync(path, JSON.stringify({ ...identity, ...record }) + '\n', {
        mode: 0o600,
      });
    } catch (error) {
      diagnostics.add(
        `worker lifecycle record could not be written: ${(error as Error).message}`,
      );
      process.stderr.write(
        `hunin: Deno worker lifecycle record unavailable: ${(error as Error).message}\n`,
      );
    }
  };
  let session: inspector.Session | undefined, deadline: number | undefined;
  let finishing = false,
    barrierCompleted = false,
    stopped = false;
  function post(
    method: 'Runtime.evaluate',
    params: inspector.Runtime.EvaluateParameterType,
  ): Promise<inspector.Runtime.EvaluateReturnType>;
  function post(
    method: 'Runtime.getProperties',
    params: inspector.Runtime.GetPropertiesParameterType,
  ): Promise<InspectorProperties>;
  function post(
    method: 'Runtime.releaseObjectGroup',
    params: inspector.Runtime.ReleaseObjectGroupParameterType,
  ): Promise<void>;
  function post(method: string, params: object): Promise<unknown> {
    return new Promise((resolve, reject) =>
      session!.post(method, params, (error, value) =>
        error ? reject(error) : resolve(value),
      ),
    );
  }
  append({
    status: 'pending',
    event: 'started',
    runtime_version: Deno!.version.deno,
    scope: 'main_isolate_web_workers',
  });

  async function identify(worker: Worker, registration: WorkerRegistration) {
    const key = `__hunin_worker_${registration.registration_id.replaceAll('-', '_')}`;
    const objectGroup = key;
    try {
      if (!session) {
        session = new inspector.Session();
        session.connect();
      }
      (globalThis as unknown as Record<string, unknown>)[key] = worker;
      const result = await post('Runtime.evaluate', {
        expression: `globalThis[${JSON.stringify(key)}]`,
        objectGroup,
      });
      if (!result.result?.objectId)
        throw Error('Inspector did not return a worker object identity');
      const properties = await post('Runtime.getProperties', {
        objectId: result.result.objectId,
        ownProperties: true,
      });
      // Pinned Deno's Worker.#id is the op_create_worker WorkerId, whose Display
      // implementation supplies the native profile's worker-N filename suffix.
      // Unknown layouts fail explicitly; constructor order is never an ID.
      const ids =
        properties.privateProperties?.filter(
          (property) => property.name === '#id',
        ) ?? [];
      const id = ids[0]?.value?.value;
      if (
        ids.length !== 1 ||
        typeof id !== 'number' ||
        !Number.isSafeInteger(id) ||
        id <= 0
      )
        throw Error('Deno Worker.#id is unavailable or invalid');
      registration.worker_id = id;
    } catch (error) {
      registration.error = `worker identity unavailable: ${(error as Error).message}`;
      diagnostics.add(registration.error);
    } finally {
      delete (globalThis as unknown as Record<string, unknown>)[key];
      if (session) {
        try {
          await post('Runtime.releaseObjectGroup', { objectGroup });
        } catch {
          /* Session failure is already retained above. */
        }
      }
      registration.settled = true;
    }
  }
  const OriginalWorker = globalThis.Worker;
  globalThis.Worker = class Worker extends OriginalWorker {
    constructor(...args: ConstructorParameters<typeof OriginalWorker>) {
      super(...args);
      const registration: WorkerRegistration = {
        registration_id: randomUUID(),
        worker_id: null,
        settled: false,
      };
      registrations.push(registration);
      // User beforeExit hooks can resume application work after a completed
      // barrier. Its next real shutdown is a new phase, with a fresh budget.
      // An unfinished/expired barrier never resets its deadline on reentry.
      if (barrierCompleted) deadline = undefined;
      barrierCompleted = false;
      append({
        status: 'pending',
        event: 'worker_registered',
        registration_id: registration.registration_id,
      });
      void identify(this, registration);
    }
  };

  function readProfile(
    filename: string,
  ): Omit<WorkerProfile, 'worker_id'> | { pending_error: string } | null {
    const fd = openSync(join(directory, filename), 'r');
    try {
      const stat = fstatSync(fd);
      if (!stat.isFile()) throw Error('profile path is not a regular file');
      if (stat.size > MAX_PROFILE_BYTES)
        throw Error('profile exceeds the64MiB lifecycle validation limit');
      // Bound the read even if a native writer grows the file after fstat.
      const buffer = Buffer.alloc(stat.size + 1);
      let length = 0;
      while (length < buffer.length) {
        const size = readSync(fd, buffer, length, buffer.length - length, null);
        if (!size) break;
        length += size;
      }
      if (length !== stat.size || fstatSync(fd).size !== stat.size) return null;
      const bytes = buffer.subarray(0, length);
      let profile;
      try {
        profile = JSON.parse(bytes.toString('utf8'));
      } catch (error) {
        return { pending_error: (error as Error).message };
      }
      if (
        !Array.isArray(profile.nodes) ||
        !Array.isArray(profile.samples) ||
        !Array.isArray(profile.timeDeltas) ||
        profile.samples.length !== profile.timeDeltas.length
      ) {
        throw Error('profile does not contain complete V8 sample arrays');
      }
      return {
        filename,
        sha256: createHash('sha256').update(bytes).digest('hex'),
      };
    } finally {
      closeSync(fd);
    }
  }

  async function flush() {
    if (finishing) return;
    finishing = true;
    deadline ??= performance.now() + timeoutMs;
    try {
      if (!registrations.length && !diagnostics.size) {
        barrierCompleted = true;
        return;
      }
      for (;;) {
        if (performance.now() >= deadline) break;
        const names = registrations.length ? readdirSync(directory) : [];
        const ids = new Set<number>();
        for (const registration of registrations) {
          if (performance.now() >= deadline) break;
          if (!registration.settled || registration.error) continue;
          const id = registration.worker_id!;
          if (ids.has(id)) throw Error(`duplicate Deno worker identity ${id}`);
          ids.add(id);
          if (profiles.has(id)) continue;
          const expected = new RegExp(
            `^CPU\\.\\d+\\.${process.pid}\\.worker-${id}\\.cpuprofile$`,
          );
          const matches = names.filter((name) => expected.test(name));
          if (matches.length > 1)
            throw Error(`ambiguous CPU profile for worker-${id}`);
          if (!matches.length) {
            registration.pending_error = 'profile was not created';
            continue;
          }
          const profile = readProfile(matches[0]);
          if (!profile || 'pending_error' in profile) {
            registration.pending_error =
              profile?.pending_error ?? 'profile is still being written';
            continue;
          }
          profiles.set(id, { worker_id: id, ...profile });
        }
        // Synchronous filesystem I/O and JSON.parse cannot be preempted. Do
        // not start another unit or claim a timely barrier after they overrun.
        if (performance.now() >= deadline) break;
        if (
          registrations.every(
            (registration) =>
              registration.settled &&
              registration.worker_id &&
              profiles.has(registration.worker_id),
          ) &&
          !diagnostics.size
        ) {
          barrierCompleted = true;
          return;
        }
        if (
          registrations.every((registration) => registration.settled) &&
          diagnostics.size
        )
          return;
        if (performance.now() >= deadline) break;
        // A real referenced timer is needed: a Promise alone does not keep
        // Deno alive after the native worker control channel closes. This polls
        // completion of the same capture; it never retries the target.
        await new Promise((resolve) =>
          setTimeout(resolve, Math.min(5, deadline! - performance.now())),
        );
      }
      diagnostics.add(
        `worker profile lifecycle deadline expired after ${timeoutMs}ms`,
      );
      for (const registration of registrations) {
        if (!registration.worker_id || !profiles.has(registration.worker_id)) {
          diagnostics.add(
            `worker-${registration.worker_id ?? 'unknown'} profile flush did not complete within ${timeoutMs}ms: ${registration.error ?? registration.pending_error ?? 'identity probe did not complete'}`,
          );
        }
      }
    } catch (error) {
      diagnostics.add(
        `worker profile lifecycle unavailable: ${(error as Error).message}`,
      );
    } finally {
      finishing = false;
      if (!barrierCompleted) stopped = true;
    }
  }
  process.on('beforeExit', () => {
    if (!barrierCompleted && !stopped) void flush();
  });
  process.once('exit', () => {
    try {
      if (!barrierCompleted)
        diagnostics.add(
          'Deno worker profile lifecycle did not finish before process exit',
        );
      append({
        status: barrierCompleted ? 'complete' : 'incomplete',
        event: 'finished',
        workers: registrations.map(({ registration_id, worker_id }) => ({
          registration_id,
          worker_id,
        })),
        profiles: [...profiles.values()],
        diagnostics: [...diagnostics],
      });
      if (!barrierCompleted)
        process.stderr.write(
          `hunin: Deno worker collection incomplete: ${[...diagnostics].join('; ')}\n`,
        );
    } finally {
      session?.disconnect();
    }
  });
}
