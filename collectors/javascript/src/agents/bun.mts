import inspector from 'node:inspector';
import { writeFileSync, mkdirSync } from 'node:fs';
import { basename, join } from 'node:path';
import { createRequire, syncBuiltinESMExports } from 'node:module';
import { fileURLToPath } from 'node:url';
import { createHash, randomUUID } from 'node:crypto';
import process from 'node:process';
import type { WorkerOptions as NodeWorkerOptions } from 'node:worker_threads';
import { BunProducerManifest } from './bun/manifest.mjs';
const require = createRequire(import.meta.url);
const threads: typeof import('node:worker_threads') = require('node:worker_threads');
const preload = fileURLToPath(import.meta.url);
const directory = process.env.HUNIN_JS_PROFILE_DIR;
if (directory) {
  mkdirSync(directory, { recursive: true });
  const captureDirectory = directory;
  const producer = randomUUID();
  const sessionId = process.env.HUNIN_BUN_SESSION;
  const runId = process.env.HUNIN_RUN_ID;
  const rootRegistrationId = threads.isMainThread
    ? process.env.HUNIN_BUN_ROOT_REGISTRATION
    : undefined;
  if (threads.isMainThread) delete process.env.HUNIN_BUN_ROOT_REGISTRATION;
  const manifest =
    sessionId && runId
      ? new BunProducerManifest({
          directory,
          runId,
          sessionId,
          rootRegistrationId,
          producerId: producer,
          pid: process.pid,
          tid: threads.threadId,
          isMainThread: threads.isMainThread,
        })
      : undefined;
  if (!manifest)
    process.stderr.write(
      'hunin: Bun producer lifecycle unavailable: capture session identity is missing\n',
    );
  const options = <T extends object>(
    value: T | undefined,
  ): T & { preload: string[] } =>
    ({
      ...value,
      preload: [
        preload,
        ...[
          (value as (T & { preload?: string | string[] }) | undefined)?.preload,
        ]
          .flat()
          .filter((item): item is string => Boolean(item))
          .filter((item) => item !== preload),
      ],
    }) as T & { preload: string[] };
  const stopWorker = <T,>(
    worker: { threadId: number },
    terminate: () => T | PromiseLike<T>,
  ) => {
    const channel = new BroadcastChannel(
      `hunin-cpu-${process.pid}-${worker.threadId}`,
    );
    return new Promise<T>((resolve) => {
      let done = false;
      const finish = () => {
        if (done) return;
        done = true;
        clearTimeout(timeout);
        channel.close();
        resolve(terminate());
      };
      channel.onmessage = (event) => {
        if (event.data === 'flushed') finish();
      };
      const timeout = setTimeout(finish, 100);
      channel.postMessage('flush-stop');
    });
  };
  const OriginalWebWorker = globalThis.Worker;
  globalThis.Worker = class extends OriginalWebWorker {
    // Bun exposes its runtime thread ID through its Web Worker facade.
    declare readonly threadId: number;

    constructor(script: string | URL, value?: WorkerOptions) {
      super(script, options(value));
      manifest?.registerWorker(this.threadId);
    }
    terminate() {
      void stopWorker(this, () => super.terminate());
    }
  };
  const OriginalNodeWorker = threads.Worker;
  threads.Worker = class extends OriginalNodeWorker {
    constructor(script: string | URL, value?: NodeWorkerOptions) {
      super(script, options(value));
      manifest?.registerWorker(this.threadId);
    }
    terminate() {
      return stopWorker(this, () => super.terminate());
    }
  };
  syncBuiltinESMExports();
  // Bun does not refresh an already-created worker_threads ESM binding here.
  // Install the constructor hook before source capture imports threadId.
  const { captureBunProfile } = await import('../sources-preload.mjs');
  const session = new inspector.Session();
  session.connect();
  function post(
    method: 'Profiler.stop',
  ): Promise<inspector.Profiler.StopReturnType>;
  function post(method: 'Profiler.enable' | 'Profiler.start'): Promise<void>;
  function post(method: string): Promise<unknown> {
    return new Promise((resolve, reject) =>
      session.post(method, (error, result) =>
        error ? reject(error) : resolve(result),
      ),
    );
  }
  await post('Profiler.enable');
  await post('Profiler.start');
  let sequence = 0,
    stopping = false,
    finished = false;
  let pending: Promise<boolean> | undefined;
  async function save(restart: boolean): Promise<boolean> {
    if (!restart) stopping = true;
    if (pending) {
      const persisted = await pending;
      if (restart || finished) return persisted;
    }
    if (finished) return manifest?.isComplete ?? true;
    pending = (async () => {
      try {
        const { profile } = await post('Profiler.stop');
        captureBunProfile(profile, producer);
        const path = join(
          captureDirectory,
          `CPU.${process.pid}.${threads.threadId}.${producer}.${++sequence}.cpuprofile`,
        );
        const profileJson = JSON.stringify(profile);
        const clockJson = JSON.stringify({
          profile: path,
          pid: process.pid,
          tid: String(threads.threadId),
          profile_time_origin_us: 0,
          uncertainty_us: 1,
          clock_source: 'JavaScriptCore epoch microseconds',
        });
        writeFileSync(path, profileJson, { mode: 0o600 });
        writeFileSync(path + '.meta.json', clockJson, { mode: 0o600 });
        manifest?.recordSegment({
          sequence,
          filename: basename(path),
          sha256: createHash('sha256').update(profileJson).digest('hex'),
          clock_sha256: createHash('sha256').update(clockJson).digest('hex'),
        });
        if (restart && !stopping) await post('Profiler.start');
        else {
          finished = true;
          session.disconnect();
          return manifest?.finish() ?? true;
        }
        return true;
      } catch (error) {
        manifest?.fail(error);
        if (!restart || stopping) manifest?.finish();
        process.stderr.write(
          `hunin: Bun CPU collection failed: ${(error as Error).message}\n`,
        );
        return false;
      }
    })();
    try {
      return await pending;
    } finally {
      pending = undefined;
    }
  }
  const control = new BroadcastChannel(
    `hunin-cpu-${process.pid}-${threads.threadId}`,
  );
  (control as BroadcastChannel & { unref(): void }).unref();
  control.onmessage = (event) => {
    if (event.data === 'flush-stop') {
      clearInterval(timer);
      void save(false).then((persisted) =>
        control.postMessage(persisted ? 'flushed' : 'flush-failed'),
      );
    }
  };
  const timer = setInterval(() => save(true), 50);
  timer.unref();
  process.once('beforeExit', () => {
    clearInterval(timer);
    void save(false);
  });
}
