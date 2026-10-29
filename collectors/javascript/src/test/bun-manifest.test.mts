import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { BunProducerManifest } from '../agents/bun/manifest.mjs';

function producer(directory: string) {
  return new BunProducerManifest({
    directory,
    runId: 'recording',
    sessionId: 'session',
    producerId: 'producer',
    rootRegistrationId: 'root',
    pid: 123,
    tid: 0,
    isMainThread: true,
  });
}

const segment = {
  sequence: 1,
  filename: 'CPU.123.0.producer.1.cpuprofile',
  sha256: 'profile-digest',
  clock_sha256: 'clock-digest',
};

test('Bun producer loss remains incomplete after a later segment succeeds', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-bun-loss-'));
  try {
    const manifest = producer(directory);
    manifest.fail(Error('earlier segment could not be saved'));
    manifest.recordSegment(segment);
    assert.equal(manifest.finish(), false);
    assert.equal(manifest.isComplete, false);
    const report: {
      status: string;
      diagnostics: string[];
      segments: unknown[];
    } = JSON.parse(
      readFileSync(
        join(directory, 'bun-producer-session-producer.json'),
        'utf8',
      ),
    );
    assert.equal(report.status, 'incomplete');
    assert.deepEqual(report.diagnostics, [
      'earlier segment could not be saved',
    ]);
    assert.equal(report.segments.length, 1);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('Bun producer cannot retain a complete snapshot after late worker creation', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-bun-late-worker-'));
  try {
    const manifest = producer(directory);
    manifest.recordSegment(segment);
    assert.equal(manifest.finish(), true);
    manifest.registerWorker(1);
    assert.equal(manifest.isComplete, false);
    const report: {
      status: string;
      diagnostics: string[];
      workers: Array<{ thread_id: number }>;
    } = JSON.parse(
      readFileSync(
        join(directory, 'bun-producer-session-producer.json'),
        'utf8',
      ),
    );
    assert.equal(report.status, 'incomplete');
    assert.deepEqual(
      report.workers.map((worker) => worker.thread_id),
      [1],
    );
    assert.match(report.diagnostics.join(' '), /created after.*finalized/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
