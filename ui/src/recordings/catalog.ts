import { operation, type Run, type Operations } from '../api.js';

type RunPage = Operations['runs.list']['output'];
export type CatalogRead = { current: () => boolean; error?: unknown };

/** The sidebar's bounded recording pages have their own replaceable read lifetime. */
export class RecordingCatalog {
  private items: Run[] = [];
  private cursor: string | undefined;
  private generation = 0;
  private controller: AbortController | undefined;

  get runs(): readonly Run[] {
    return this.items;
  }

  get hasOlder(): boolean {
    return Boolean(this.cursor);
  }

  pause(): void {
    this.controller?.abort();
  }

  refresh(retain: () => string): Promise<CatalogRead> {
    this.pause();
    this.controller = new AbortController();
    this.generation++;
    return this.read(undefined, (page) => {
      const current = this.items.find((run) => run.id === retain());
      this.items = page.runs;
      if (current && !this.items.some((run) => run.id === current.id))
        this.items.push(current);
      this.cursor = page.next_cursor ?? undefined;
    });
  }

  older(): Promise<CatalogRead> {
    if (!this.cursor || !this.controller)
      return Promise.resolve({ current: () => false });
    return this.read(this.cursor, (page) => {
      const unique = new Map(this.items.map((run) => [run.id, run]));
      for (const run of page.runs)
        if (!unique.has(run.id)) unique.set(run.id, run);
      this.items = [...unique.values()];
      this.cursor = page.next_cursor ?? undefined;
    });
  }

  ensure(run: Run): boolean {
    if (this.items.some((value) => value.id === run.id)) return false;
    this.items.push(run);
    return true;
  }

  observe(run: Run): void {
    this.items = this.items.map((value) => (value.id === run.id ? run : value));
  }

  private async read(
    cursor: string | undefined,
    apply: (page: RunPage) => void,
  ): Promise<CatalogRead> {
    const generation = this.generation;
    const signal = this.controller!.signal;
    const current = () => generation === this.generation && !signal.aborted;
    try {
      const page = await operation('runs.list', { limit: 100, cursor }, signal);
      if (current()) apply(page);
      return { current };
    } catch (error) {
      return { current, error };
    }
  }
}
