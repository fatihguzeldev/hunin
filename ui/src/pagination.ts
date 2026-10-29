export type Page<T> = { items: T[]; next_cursor?: string | null };

/** Keep opaque cursors and commit navigation only after its request succeeds. */
export class PagedQuery<T> {
  page: Page<T> = { items: [] };
  private cursor?: string;
  private previousCursors: (string | undefined)[] = [];
  private version = 0;

  constructor(
    private fetchPage: (cursor?: string) => Promise<Page<T>>,
    private signal?: AbortSignal,
  ) {}

  get hasPrevious(): boolean {
    return this.previousCursors.length > 0;
  }

  refresh(reset = false): Promise<boolean> {
    return this.load(
      reset ? undefined : this.cursor,
      reset ? [] : this.previousCursors,
    );
  }

  next(): Promise<boolean> {
    return this.page.next_cursor
      ? this.load(this.page.next_cursor, [...this.previousCursors, this.cursor])
      : Promise.resolve(false);
  }

  previous(): Promise<boolean> {
    return this.hasPrevious
      ? this.load(
          this.previousCursors.at(-1),
          this.previousCursors.slice(0, -1),
        )
      : Promise.resolve(false);
  }

  private async load(
    cursor: string | undefined,
    previous: (string | undefined)[],
  ): Promise<boolean> {
    if (this.signal?.aborted) return false;
    const version = ++this.version;
    try {
      const page = await this.fetchPage(cursor);
      if (version !== this.version || this.signal?.aborted) return false;
      this.page = page;
      this.cursor = cursor;
      this.previousCursors = previous;
      return true;
    } catch (error) {
      if (version === this.version && !this.signal?.aborted) throw error;
      return false;
    }
  }
}
