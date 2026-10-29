import { randomUUID } from 'node:crypto';
import { renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

export interface BunSegment {
  sequence: number;
  filename: string;
  sha256: string;
  clock_sha256: string;
}

interface ProducerIdentity {
  directory: string;
  runId: string;
  sessionId: string;
  rootRegistrationId?: string;
  producerId: string;
  pid: number;
  tid: number;
  isMainThread: boolean;
}

/** One isolate owns this snapshot; a missing final snapshot remains incomplete. */
export class BunProducerManifest {
  private readonly segments: BunSegment[] = [];
  private readonly workers: Array<{
    registration_id: string;
    thread_id: number;
  }> = [];
  private readonly diagnostics = new Set<string>();
  private status: 'recording' | 'complete' | 'incomplete' = 'recording';
  private completeOnDisk = false;

  constructor(private readonly identity: ProducerIdentity) {
    this.persist();
  }

  get isComplete(): boolean {
    return this.completeOnDisk;
  }

  registerWorker(threadId: number): void {
    if (!Number.isSafeInteger(threadId) || threadId <= 0) {
      this.fail(
        Error(
          'worker constructor did not expose a valid runtime thread identity',
        ),
      );
      return;
    }
    // Bun's Node and Web Worker facades can refer to the same worker thread.
    if (this.workers.some((worker) => worker.thread_id === threadId)) return;
    this.workers.push({ registration_id: randomUUID(), thread_id: threadId });
    if (this.status !== 'recording') {
      this.fail(Error('worker was created after this producer finalized'));
      this.status = 'incomplete';
      this.persist();
    }
  }

  recordSegment(segment: BunSegment): void {
    this.segments.push(segment);
  }

  fail(error: unknown): void {
    this.completeOnDisk = false;
    this.diagnostics.add(
      error instanceof Error ? error.message : String(error),
    );
  }

  finish(): boolean {
    if (!this.segments.length)
      this.fail(Error('producer has no persisted CPU segment'));
    this.status = this.diagnostics.size ? 'incomplete' : 'complete';
    const persisted = this.persist();
    this.completeOnDisk = persisted && this.status === 'complete';
    return this.completeOnDisk;
  }

  private persist(): boolean {
    const owner = this.identity;
    const path = join(
      owner.directory,
      `bun-producer-${owner.sessionId}-${owner.producerId}.json`,
    );
    const temporary = path + '.tmp';
    try {
      writeFileSync(
        temporary,
        JSON.stringify({
          schema_version: 1,
          runtime: 'bun',
          run_id: owner.runId,
          session_id: owner.sessionId,
          producer_id: owner.producerId,
          root_registration_id: owner.rootRegistrationId ?? null,
          pid: owner.pid,
          tid: owner.tid,
          is_main_thread: owner.isMainThread,
          status: this.status,
          segments: this.segments,
          workers: this.workers,
          diagnostics: [...this.diagnostics],
        }),
        { mode: 0o600 },
      );
      renameSync(temporary, path);
      return true;
    } catch (error) {
      const message = `producer lifecycle snapshot unavailable: ${(error as Error).message}`;
      this.fail(Error(message));
      process.stderr.write(`hunin: Bun ${message}\n`);
      return false;
    }
  }
}
