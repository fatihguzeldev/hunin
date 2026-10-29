type SessionStorage = Pick<Storage, 'getItem' | 'setItem'>;
export type WatchedAttempt = {
  runId: string;
  id: string;
  state: 'pending' | 'running' | 'unresolved';
  status?: string;
  diagnostic?: string;
};
export type SettledAttempt = { id: string; status: string };
type Lookup = (
  runId: string,
  id: string,
  signal: AbortSignal,
) => Promise<unknown[]>;
const key = 'hunin.analysis-watch.v1';
const terminal = new Set([
  'accepted',
  'replayed',
  'failed',
  'timed_out',
  'cancelled',
  'canceled',
  'interrupted',
]);

function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

/** Persist read interest in exact attempt handles, never job authority or history navigation. */
export class AnalysisWatch {
  private entries = new Map<string, WatchedAttempt>();
  private settled = new Set<string>();
  persistent: boolean;

  constructor(private storage?: SessionStorage) {
    this.persistent = storage !== undefined;
    let saved: string | null | undefined;
    try {
      saved = storage?.getItem(key);
    } catch {
      this.persistent = false;
    }
    if (!saved) return;
    try {
      const payload = object(JSON.parse(saved));
      if (payload?.version !== 1 || !Array.isArray(payload.attempts)) return;
      for (const value of payload.attempts) {
        const entry = object(value);
        if (
          typeof entry?.runId === 'string' &&
          entry.runId &&
          typeof entry.id === 'string' &&
          entry.id
        )
          this.entries.set(this.identity(entry.runId, entry.id), {
            runId: entry.runId,
            id: entry.id,
            state: 'pending',
          });
      }
    } catch {
      /* Malformed private read interest does not become job state. */
    }
  }

  private identity(runId: string, id: string): string {
    return JSON.stringify([runId, id]);
  }

  private persist(): void {
    try {
      this.storage?.setItem(
        key,
        JSON.stringify({
          version: 1,
          attempts: [...this.entries.values()].map(({ runId, id }) => ({
            runId,
            id,
          })),
        }),
      );
    } catch {
      this.persistent = false;
    }
  }

  track(runId: string, id: string): boolean {
    const key = this.identity(runId, id);
    if (!runId || !id || this.entries.has(key) || this.settled.has(key))
      return false;
    this.entries.set(key, { runId, id, state: 'pending' });
    this.persist();
    return true;
  }

  forRun(runId: string): WatchedAttempt[] {
    return [...this.entries.values()].filter((entry) => entry.runId === runId);
  }

  async check(
    runId: string,
    lookup: Lookup,
    signal: AbortSignal,
    force = false,
  ): Promise<SettledAttempt[]> {
    const completed: SettledAttempt[] = [];
    for (const entry of this.forRun(runId)) {
      if (signal.aborted) break;
      if (!force && entry.state === 'unresolved') continue;
      try {
        const results = await lookup(runId, entry.id, signal);
        if (signal.aborted) break;
        const row = results.length === 1 ? object(results[0]) : undefined;
        if (
          !row ||
          row.id !== entry.id ||
          row.run_id !== runId ||
          typeof row.status !== 'string'
        ) {
          entry.state = 'unresolved';
          entry.diagnostic =
            results.length === 0
              ? 'This attempt is not available in the recording. Absence does not establish completion.'
              : 'The service did not return exactly this owned attempt. Its status remains unconfirmed.';
          continue;
        }
        entry.status = row.status;
        entry.diagnostic = undefined;
        if (row.status === 'running') entry.state = 'running';
        else if (terminal.has(row.status)) {
          const key = this.identity(runId, entry.id);
          this.entries.delete(key);
          this.settled.add(key);
          completed.push({ id: entry.id, status: row.status });
          this.persist();
        } else {
          entry.state = 'unresolved';
          entry.diagnostic =
            'This status is not recognized as running or terminal. Check the retained attempt before drawing a conclusion.';
        }
      } catch (error) {
        if (signal.aborted) break;
        entry.state = 'unresolved';
        entry.diagnostic = `Attempt lookup failed. ${error instanceof Error ? error.message : String(error)}`;
      }
    }
    return completed;
  }
}
