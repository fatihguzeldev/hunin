import { Investigations } from './state.js';

type Browser = {
  history: Pick<
    History,
    'state' | 'pushState' | 'replaceState' | 'length' | 'go'
  >;
  location: Pick<Location, 'href'>;
};

/** Older private history entries used a boolean for the first source window. */
export function sourceReturnDepth(value: unknown): number {
  if (value === true) return 1;
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0
    ? value
    : 0;
}

/** Browser entries own navigation snapshots; selected result identity stays in Investigations. */
export class InvestigationHistory {
  constructor(
    private investigations: Investigations,
    private browser: Browser = window,
    private storageScope?: string,
  ) {}

  state(): { runId: string; saved: unknown; sourceReturn: number } | undefined {
    const value: unknown = this.browser.history.state;
    if (!value || typeof value !== 'object' || !('hunin' in value))
      return undefined;
    const state = value.hunin;
    if (
      !state ||
      typeof state !== 'object' ||
      !('runId' in state) ||
      typeof state.runId !== 'string' ||
      !('saved' in state) ||
      (this.storageScope !== undefined &&
        (!('storageScope' in state) ||
          state.storageScope !== this.storageScope))
    )
      return undefined;
    return {
      runId: state.runId,
      saved: state.saved,
      sourceReturn: sourceReturnDepth(
        'sourceReturn' in state ? state.sourceReturn : undefined,
      ),
    };
  }

  restore(runId: string): void {
    const state = this.state();
    if (runId && state?.runId === runId)
      this.investigations.restore(runId, state.saved);
  }

  returnToView(runId: string): boolean {
    const entry = this.state();
    const depth = entry?.runId === runId ? entry.sourceReturn : 0;
    if (depth > 0 && depth < this.browser.history.length) {
      this.browser.history.go(-depth);
      return true;
    }
    return false;
  }

  write(
    selection: { runId: string; view: string },
    mode: 'push' | 'replace' = 'replace',
    sourceEntry = 0,
  ): void {
    const { runId, view } = selection;
    const url = new URL(this.browser.location.href);
    if (runId) url.searchParams.set('run', runId);
    else url.searchParams.delete('run');
    url.searchParams.set('view', view);
    const previous = this.state();
    const source = runId ? this.investigations.forRun(runId).source : undefined;
    const state = {
      hunin: {
        storageScope: this.storageScope,
        runId,
        saved: runId ? this.investigations.capture(runId) : undefined,
        sourceReturn: source
          ? sourceEntry ||
            (mode === 'replace' && previous?.runId === runId
              ? previous.sourceReturn
              : 0)
          : 0,
      },
    };
    this.browser.history[mode === 'push' ? 'pushState' : 'replaceState'](
      state,
      '',
      url,
    );
    if (runId) this.investigations.persist(runId);
  }
}
