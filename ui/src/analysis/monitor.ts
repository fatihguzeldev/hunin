import { operation } from '../api.js';
import { AnalysisWatch, type SettledAttempt } from './watch.js';

/** Own lookup interest independently of foreground views and durable jobs. */
export class AttemptMonitor {
  private runId = '';
  private epoch = 0;
  private controller: AbortController | undefined;
  private checking: AbortController | undefined;
  private force = false;
  private poll: ReturnType<typeof setTimeout> | undefined;

  constructor(
    private watch: AnalysisWatch,
    private actions: {
      updated: (runId: string) => void;
      completed: (runId: string, attempts: SettledAttempt[]) => void;
    },
  ) {}

  pause(): void {
    clearTimeout(this.poll);
    this.controller?.abort();
    this.epoch++;
    this.checking = undefined;
    this.force = false;
    this.runId = '';
  }

  select(runId: string): void {
    if (this.runId === runId) return;
    this.pause();
    this.runId = runId;
    if (runId) this.schedule(true);
  }

  follow(runId: string, id: string): void {
    if (this.watch.track(runId, id) && this.runId === runId) {
      this.actions.updated(runId);
      this.schedule();
    }
  }

  recheck(): void {
    this.schedule(true);
  }

  private schedule(force = false, delay = 0): void {
    clearTimeout(this.poll);
    if (!this.runId) return;
    this.poll = setTimeout(() => {
      void this.check(force);
    }, delay);
  }

  private async check(force: boolean): Promise<void> {
    const runId = this.runId;
    if (!runId || !this.watch.forRun(runId).length) return;
    if (this.checking && !this.checking.signal.aborted) {
      this.force ||= force;
      return;
    }
    this.controller?.abort();
    const controller = new AbortController();
    this.controller = controller;
    this.checking = controller;
    const generation = ++this.epoch;
    const completed = await this.watch.check(
      runId,
      async (runId, id, signal) => {
        const page = await operation(
          'analyses.results',
          { run_id: runId, execution_id: id, limit: 1 },
          signal,
        );
        return page.results;
      },
      controller.signal,
      force,
    );
    if (this.checking === controller) this.checking = undefined;
    if (
      controller.signal.aborted ||
      generation !== this.epoch ||
      runId !== this.runId
    )
      return;
    this.actions.updated(runId);
    if (completed.length) this.actions.completed(runId, completed);
    if (this.force) {
      this.force = false;
      this.schedule(true);
    } else if (
      this.watch.forRun(runId).some((attempt) => attempt.state !== 'unresolved')
    )
      this.schedule(false, 1_500);
  }
}
