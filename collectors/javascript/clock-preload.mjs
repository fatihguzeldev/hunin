import inspector from 'node:inspector';
import process from 'node:process';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { threadId } from 'node:worker_threads';
import './sources-preload.mjs';

// Calibrate the V8 profiler's monotonic domain against epoch time in the same
// isolate. A bounded handshake replaces assumptions about hrtime polyfills.
let directory;
try { directory = process.env.HUNIN_JS_PROFILE_DIR; } catch (error) {
  process.stderr.write(`hunin: CPU clock calibration unavailable: ${error.message}\n`);
}
if (directory) {
  const session = new inspector.Session();
  try {
    session.connect();
    const post = method => new Promise((resolve, reject) => session.post(method, (error, value) => error ? reject(error) : resolve(value)));
    await post('Profiler.enable');
    const before = performance.timeOrigin * 1000 + performance.now() * 1000;
    await post('Profiler.start');
    const after = performance.timeOrigin * 1000 + performance.now() * 1000;
    const { profile } = await post('Profiler.stop');
    writeFileSync(join(directory, `clock-${process.pid}-${threadId}.meta.json`), JSON.stringify({
      pid: process.pid, tid: String(threadId), runtime: globalThis.Deno ? 'deno' : 'node',
      profile_time_origin_us: Math.round((before + after) / 2 - profile.startTime),
      uncertainty_us: Math.ceil((after - before) / 2),
      clock_source: 'V8 Inspector Profiler.start bracketed by performance epoch clock',
    }), { mode: 0o600 });
  } catch (error) {
    process.stderr.write(`hunin: CPU clock calibration failed: ${error.message}\n`);
  } finally { session.disconnect(); }
}
if (globalThis.Deno && directory) {
  try {
    const { installDenoWorkerBarrier } = await import('./deno-workers.mjs');
    installDenoWorkerBarrier({ directory, runId: process.env.HUNIN_RUN_ID, producerId: process.env.HUNIN_DENO_WORKER_SESSION });
  } catch (error) {
    // The prelaunch planned record remains incomplete. A target's explicit
    // permission denial must not turn collector setup into a target failure.
    process.stderr.write(`hunin: Deno worker lifecycle setup unavailable: ${error.message}\n`);
  }
}
