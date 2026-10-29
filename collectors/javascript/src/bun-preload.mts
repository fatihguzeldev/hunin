import inspector from 'node:inspector';
import { writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { createRequire, syncBuiltinESMExports } from 'node:module';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import process from 'node:process';
import type { WorkerOptions as NodeWorkerOptions } from 'node:worker_threads';
import { captureBunProfile } from './sources-preload.mjs';
const require = createRequire(import.meta.url);
const threads: typeof import('node:worker_threads') = require('node:worker_threads');
const preload = fileURLToPath(import.meta.url);
const directory = process.env.HUNIN_JS_PROFILE_DIR;
if (directory) {
  mkdirSync(directory, { recursive: true });
  const captureDirectory = directory;
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
    // This extra native identity is specific to Bun's Web Worker.
    declare readonly threadId: number;

    constructor(script: string | URL, value?: WorkerOptions) {
      super(script, options(value));
    }
    terminate() {
      void stopWorker(this, () => super.terminate());
    }
  };
  const OriginalNodeWorker = threads.Worker;
  threads.Worker = class extends OriginalNodeWorker {
    constructor(script: string | URL, value?: NodeWorkerOptions) {
      super(script, options(value));
    }
    terminate() {
      return stopWorker(this, () => super.terminate());
    }
  };
  syncBuiltinESMExports();
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
  const producer = randomUUID();
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
    if (finished) return true;
    pending = (async () => {
      try {
        const { profile } = await post('Profiler.stop');
        captureBunProfile(profile, producer);
        const path = join(
          captureDirectory,
          `CPU.${process.pid}.${threads.threadId}.${producer}.${++sequence}.cpuprofile`,
        );
        writeFileSync(path, JSON.stringify(profile), { mode: 0o600 });
        writeFileSync(
          path + '.meta.json',
          JSON.stringify({
            profile: path,
            pid: process.pid,
            tid: String(threads.threadId),
            profile_time_origin_us: 0,
            uncertainty_us: 1,
            clock_source: 'JavaScriptCore epoch microseconds',
          }),
          { mode: 0o600 },
        );
        if (restart && !stopping) await post('Profiler.start');
        else {
          finished = true;
          session.disconnect();
        }
        return true;
      } catch (error) {
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
