import { Investigations, type Investigation } from './investigation';
import { runSource } from './run';
import { operation, type Run, type Summary, type Operations } from './api';
import { badge, button, el, empty, errorView, loading } from './dom';
import { date } from './format';
import { recordDialog } from './recording';
import { capabilities } from './views/capabilities';
import { overview, runTone } from './views/overview';
import { scenarios } from './views/scenarios';

export type ViewContext = {
  run: Run;
  summary: Summary;
  runs: Run[];
  investigation: Investigation;
  signal: AbortSignal;
  persist: (mode?: 'push' | 'replace') => void;
  refresh: (message?: string) => void;
  source: (path: string, line?: number | null, codeId?: string | null) => void;
};
export type View = {
  name: string;
  render: (context: ViewContext) => Promise<HTMLElement>;
};

export class Workspace {
  private runs: Run[] = [];
  private selected = new URLSearchParams(location.search).get('run') ?? '';
  private tab = new URLSearchParams(location.search).get('view') ?? 'overview';
  private rail = el('div', { class: 'run-list', 'aria-label': 'Recordings' });
  private content = el('main', { class: 'workspace', id: 'main' });
  private epoch = 0;
  private reloadEpoch = 0;
  private query = '';
  private nextCursor: string | undefined;
  private focusSelectedTab = false;
  private poll: ReturnType<typeof setTimeout> | undefined;
  private views: Record<string, View>;
  private investigations: Investigations;
  private renderController: AbortController | undefined;
  private pendingInvalidation: 'changed' | 'unconfirmed' | undefined;
  private returningFromSource = false;
  private messages = new Map<string, string>();

  constructor(
    views: Record<string, View>,
    private sourceView: (
      run: Run,
      path: string,
      line?: number,
      codeId?: string,
      signal?: AbortSignal,
    ) => Promise<HTMLElement>,
  ) {
    let storage: Storage | undefined;
    try {
      storage = window.sessionStorage;
    } catch {
      /* Private state remains in memory. */
    }
    this.investigations = new Investigations(storage);
    this.views = {
      overview: {
        name: 'Overview',
        render: (context) =>
          overview(context.run, context.summary, context.source),
      },
      ...views,
    };
    this.normalizeTab();
    this.restoreHistory();
    window.addEventListener('popstate', () => {
      const wasSource = Boolean(
        this.selected && this.investigations.forRun(this.selected).source,
      );
      const params = new URLSearchParams(location.search);
      this.selected = params.get('run') ?? '';
      this.tab = params.get('view') ?? 'overview';
      this.normalizeTab();
      this.restoreHistory();
      this.returningFromSource =
        wasSource && !this.investigations.forRun(this.selected).source;
      this.renderRail();
      void this.render();
    });
  }

  private normalizeTab(): void {
    if (
      ![...Object.keys(this.views), 'capabilities', 'scenarios'].includes(
        this.tab,
      )
    )
      this.tab = 'overview';
  }

  mount(root: HTMLElement): void {
    const search = el('input', {
      type: 'search',
      class: 'run-search',
      placeholder: 'Filter recordings',
      'aria-label': 'Filter recordings',
    });
    search.addEventListener('input', () => {
      this.query = search.value;
      this.renderRail();
    });
    root.replaceChildren(
      el('a', { class: 'skip-link', href: '#main' }, 'Skip to recording'),
      el(
        'header',
        { class: 'app-header' },
        el(
          'div',
          { class: 'brand' },
          el('img', { src: '/hunin.svg', alt: '' }),
          'hunin',
        ),
        el('span', { class: 'header-caption' }, 'profiling & observability'),
        el(
          'div',
          { class: 'header-actions' },
          button(
            'Capabilities',
            () => this.navigate('', 'capabilities'),
            'button subtle',
          ),
          button(
            'Scenarios',
            () => this.navigate('', 'scenarios'),
            'button subtle',
          ),
          button(
            '+ Record',
            () =>
              recordDialog((id) => {
                void this.reload(id);
              }),
            'button primary',
          ),
        ),
      ),
      el(
        'div',
        { class: 'shell' },
        el(
          'aside',
          { class: 'sidebar' },
          el(
            'div',
            { class: 'sidebar-title' },
            el('h2', {}, 'Recordings'),
            button(
              '↻',
              () => {
                void this.reload();
              },
              'button subtle small',
            ),
          ),
          search,
          this.rail,
        ),
        this.content,
      ),
    );
    this.rail.replaceChildren(loading('Loading recordings…'));
    void this.reload();
  }

  private async reload(select?: string): Promise<void> {
    const generation = ++this.reloadEpoch;
    const navigation = this.epoch;
    try {
      const data = await operation('runs.list', { limit: 100 });
      if (generation !== this.reloadEpoch) return;
      this.runs = data.runs;
      this.nextCursor = data.next_cursor ?? undefined;
      if (navigation !== this.epoch) {
        this.renderRail();
        return;
      }
      if (select) {
        this.selected = select;
        this.tab = 'overview';
        this.writeLocation();
      }
      if (!this.selected && !['capabilities', 'scenarios'].includes(this.tab))
        this.selected = this.runs[0]?.id ?? '';
      this.renderRail();
      await this.render();
    } catch (error) {
      if (generation === this.reloadEpoch && navigation === this.epoch)
        this.content.replaceChildren(
          errorView(error, () => {
            void this.reload();
          }),
        );
    }
  }

  private renderRail(): void {
    this.rail.replaceChildren();
    const shown = this.runs.filter((run) =>
      `${run.name} ${run.runtime} ${run.id}`
        .toLowerCase()
        .includes(this.query.toLowerCase()),
    );
    for (const run of shown) {
      const item = button(
        '',
        () =>
          this.navigate(
            run.id,
            ['capabilities', 'scenarios'].includes(this.tab)
              ? 'overview'
              : this.tab,
          ),
        `run-item ${run.id === this.selected ? 'active' : ''}`,
      );
      item.setAttribute(
        'aria-current',
        run.id === this.selected ? 'page' : 'false',
      );
      item.append(
        el('span', { class: 'run-item-title' }, run.name),
        el(
          'span',
          { class: 'run-item-info' },
          el('span', {}, run.runtime),
          badge(run.status, runTone(run)),
        ),
        el('span', { class: 'run-item-info mono' }, run.id.slice(0, 12)),
      );
      this.rail.append(item);
    }
    if (!shown.length)
      this.rail.append(
        el(
          'p',
          { class: 'sidebar-empty' },
          this.runs.length
            ? 'No matching recordings.'
            : 'No recordings yet. Record a workload to begin.',
        ),
      );
    if (this.nextCursor) {
      const older = button(
        'Load older recordings',
        () => {
          older.disabled = true;
          const cursor = this.nextCursor;
          const generation = this.reloadEpoch;
          void operation('runs.list', { limit: 100, cursor })
            .then((data) => {
              if (generation !== this.reloadEpoch) return;
              const unique = new Map(this.runs.map((run) => [run.id, run]));
              for (const run of data.runs)
                if (!unique.has(run.id)) unique.set(run.id, run);
              this.runs = [...unique.values()];
              this.nextCursor = data.next_cursor ?? undefined;
              this.renderRail();
            })
            .catch((error) => {
              if (generation === this.reloadEpoch)
                this.rail.append(
                  errorView(error, () => {
                    void this.reload();
                  }),
                );
            })
            .finally(() => {
              older.disabled = false;
            });
        },
        'button subtle small',
      );
      this.rail.append(older);
    }
  }

  private navigate(id: string, tab: string): void {
    this.selected = id;
    this.tab = tab;
    if (id) this.investigations.forRun(id).source = undefined;
    this.writeLocation('push');
    this.renderRail();
    void this.render();
  }

  private historyState():
    | { runId: string; saved: unknown; sourceReturn: boolean }
    | undefined {
    const value: unknown = history.state;
    if (!value || typeof value !== 'object' || !('hunin' in value))
      return undefined;
    const state = value.hunin;
    if (
      !state ||
      typeof state !== 'object' ||
      !('runId' in state) ||
      typeof state.runId !== 'string' ||
      !('saved' in state)
    )
      return undefined;
    return {
      runId: state.runId,
      saved: state.saved,
      sourceReturn: 'sourceReturn' in state && state.sourceReturn === true,
    };
  }

  private restoreHistory(): void {
    const state = this.historyState();
    if (this.selected && state?.runId === this.selected)
      this.investigations.restore(this.selected, state.saved);
  }

  private writeLocation(
    mode: 'push' | 'replace' = 'replace',
    sourceEntry = false,
  ): void {
    const url = new URL(location.href);
    if (this.selected) url.searchParams.set('run', this.selected);
    else url.searchParams.delete('run');
    url.searchParams.set('view', this.tab);
    const previous = this.historyState();
    const source = this.selected
      ? this.investigations.forRun(this.selected).source
      : undefined;
    const state = {
      hunin: {
        runId: this.selected,
        saved: this.selected
          ? this.investigations.capture(this.selected)
          : undefined,
        sourceReturn: Boolean(
          source &&
          (sourceEntry ||
            (mode === 'replace' &&
              previous?.runId === this.selected &&
              previous.sourceReturn)),
        ),
      },
    };
    history[mode === 'push' ? 'pushState' : 'replaceState'](state, '', url);
    if (this.selected) this.investigations.persist(this.selected);
  }

  private async render(
    snapshot?: Operations['runs.get']['output'],
  ): Promise<void> {
    const epoch = ++this.epoch;
    this.renderController?.abort();
    const controller = new AbortController();
    this.renderController = controller;
    const signal = controller.signal;
    clearTimeout(this.poll);
    this.content.replaceChildren(loading('Loading recording…'));
    try {
      if (this.tab === 'capabilities') {
        const view = await capabilities();
        if (epoch === this.epoch) {
          this.content.replaceChildren(el('div', { class: 'view' }, view));
          document.title = 'Capabilities · Hunin';
        }
        return;
      }
      if (this.tab === 'scenarios') {
        const view = await scenarios((id) => {
          this.tab = 'overview';
          void this.reload(id);
        });
        if (epoch === this.epoch) {
          this.content.replaceChildren(el('div', { class: 'view' }, view));
          document.title = 'Scenarios · Hunin';
        }
        return;
      }
      if (!this.selected) {
        document.title = 'Hunin · recordings';
        this.content.replaceChildren(
          empty(
            'Make the next change measurable',
            'Record a workload, inspect its execution, then compare the same workload after your change.',
            button(
              'Record a workload',
              () =>
                recordDialog((id) => {
                  void this.reload(id);
                }),
              'button primary',
            ),
          ),
        );
        return;
      }
      const details =
        snapshot?.run.id === this.selected
          ? snapshot
          : await operation('runs.get', { run_id: this.selected }, signal);
      if (epoch !== this.epoch || signal.aborted) return;
      const { run, summary, result_identity } = details;
      const invalidation =
        this.investigations.observeResult(run.id, result_identity) ??
        this.pendingInvalidation;
      this.pendingInvalidation = undefined;
      const investigation = this.investigations.forRun(run.id);
      if (investigation.source?.view !== this.tab)
        investigation.source = undefined;
      this.writeLocation('replace');
      if (!this.runs.some((value) => value.id === run.id)) {
        this.runs.push(run);
        this.renderRail();
      }
      const view = el(
        'section',
        {
          class: 'view',
          role: 'tabpanel',
          id: 'view-panel',
          'aria-labelledby': `tab-${this.tab}`,
          tabindex: -1,
        },
        loading('Loading view…'),
      );
      const notices = invalidation
        ? [
            el(
              'p',
              { class: 'notice', role: 'status' },
              invalidation === 'changed'
                ? 'Selected results changed. Saved observation and source locations were reset; your filters are retained.'
                : 'This service cannot confirm the saved result identity. Observation and source locations were reset; your filters are retained.',
            ),
          ]
        : [];
      const message = this.messages.get(run.id);
      if (message) {
        this.messages.delete(run.id);
        notices.push(el('p', { class: 'notice', role: 'status' }, message));
      }
      if (!this.investigations.persistent)
        notices.push(
          el(
            'p',
            { class: 'metric-note' },
            'Investigation state is remembered on this page only because browser session storage is unavailable.',
          ),
        );
      this.content.replaceChildren(this.header(run), ...notices, view);
      if (this.focusSelectedTab) {
        this.content
          .querySelector<HTMLButtonElement>(`#tab-${this.tab}`)
          ?.focus();
        this.focusSelectedTab = false;
      }
      const source = (
        path: string,
        line?: number | null,
        codeId?: string | null,
      ): void =>
        this.showSource(run, path, line ?? undefined, codeId ?? undefined);
      if (investigation.source) {
        const selected = investigation.source;
        const back = button(
          `← Back to ${this.views[this.tab]?.name ?? 'recording'}`,
          () => {
            if (this.historyState()?.sourceReturn) history.back();
            else {
              investigation.source = undefined;
              this.returningFromSource = true;
              this.writeLocation('replace');
              void this.render();
            }
          },
          'button subtle small',
        );
        const sourceBody = loading('Loading measured source…');
        view.replaceChildren(back, sourceBody);
        back.focus();
        try {
          const body = await this.sourceView(
            run,
            selected.path,
            selected.line,
            selected.codeId,
            signal,
          );
          if (epoch !== this.epoch || signal.aborted) return;
          sourceBody.replaceWith(body);
        } catch (error) {
          if (epoch === this.epoch && !signal.aborted)
            sourceBody.replaceWith(
              errorView(error, () => {
                void this.render();
              }),
            );
        }
      } else {
        const renderer = this.views[this.tab] ?? this.views.overview;
        const body = await renderer.render({
          run,
          summary,
          runs: this.runs,
          source,
          investigation,
          signal,
          persist: (mode = 'replace') => {
            if (!signal.aborted) this.writeLocation(mode);
          },
          refresh: (message) => {
            if (message) this.messages.set(run.id, message);
            if (this.selected === run.id) void this.render();
          },
        });
        if (epoch !== this.epoch || signal.aborted) return;
        view.replaceChildren(body);
        if (this.returningFromSource) {
          view.focus();
          this.returningFromSource = false;
        }
      }
      document.title = `${run.name} · Hunin`;
      if (run.status === 'running') this.pollStatus(run.id, epoch, signal);
    } catch (error) {
      if (epoch === this.epoch && !signal.aborted)
        this.content.replaceChildren(
          errorView(error, () => {
            void this.render();
          }),
        );
    }
  }

  private pollStatus(id: string, epoch: number, signal: AbortSignal): void {
    this.poll = setTimeout(() => {
      void operation('runs.get', { run_id: id }, signal)
        .then((details) => {
          if (epoch !== this.epoch || signal.aborted) return;
          const { run, result_identity } = details;
          this.runs = this.runs.map((value) =>
            value.id === run.id ? run : value,
          );
          this.renderRail();
          const invalidation = this.investigations.observeResult(
            id,
            result_identity,
          );
          if (invalidation) {
            this.pendingInvalidation = invalidation;
            void this.render(details);
          } else if (run.status === 'running') {
            this.content.firstElementChild?.replaceWith(this.header(run));
            this.pollStatus(id, epoch, signal);
          } else void this.reload();
        })
        .catch((error) => {
          if (epoch === this.epoch && !signal.aborted)
            this.content.prepend(
              errorView(error, () => {
                void this.reload();
              }),
            );
        });
    }, 2_000);
  }

  private showSource(
    run: Run,
    path: string,
    line?: number,
    codeId?: string,
  ): void {
    if (run.id !== this.selected) return;
    this.investigations.forRun(run.id).source = {
      view: this.tab,
      path,
      line,
      codeId,
    };
    this.writeLocation('push', true);
    void this.render();
  }

  private header(run: Run): HTMLElement {
    const source = runSource(run);
    const actions = el('div', { class: 'toolbar' });
    if (run.status === 'running')
      actions.append(
        button(
          'Stop recording',
          () => {
            void operation('runs.cancel', { run_id: run.id })
              .then(() => this.reload())
              .catch((error) => {
                this.content.prepend(
                  errorView(error, () => {
                    void this.reload();
                  }),
                );
              });
          },
          'button danger',
        ),
      );
    const tabs = el('div', {
      class: 'tabs',
      role: 'tablist',
      'aria-label': 'Recording views',
    });
    for (const [key, view] of Object.entries(this.views)) {
      const tab = button(view.name, () => this.navigate(run.id, key), 'tab');
      tab.setAttribute('role', 'tab');
      tab.setAttribute('aria-selected', String(this.tab === key));
      tab.setAttribute('aria-controls', 'view-panel');
      tab.setAttribute('id', `tab-${key}`);
      tab.setAttribute('tabindex', this.tab === key ? '0' : '-1');
      tab.addEventListener('keydown', (event) => {
        const keys = Object.keys(this.views);
        let index = keys.indexOf(key);
        if (event.key === 'ArrowRight') index = (index + 1) % keys.length;
        else if (event.key === 'ArrowLeft')
          index = (index + keys.length - 1) % keys.length;
        else if (event.key === 'Home') index = 0;
        else if (event.key === 'End') index = keys.length - 1;
        else return;
        event.preventDefault();
        this.focusSelectedTab = true;
        this.navigate(run.id, keys[index]);
      });
      tabs.append(tab);
    }
    return el(
      'div',
      {},
      el(
        'div',
        { class: 'run-header' },
        el('div', { class: 'eyebrow' }, 'Recording'),
        el('div', { class: 'run-heading' }, el('h1', {}, run.name), actions),
        el(
          'div',
          { class: 'run-meta' },
          badge(run.status, runTone(run)),
          el('span', {}, run.runtime),
          el(
            'span',
            {},
            date(new Date(run.started_at_us / 1_000).toISOString()),
          ),
          el(
            'code',
            {},
            source.revision?.slice(0, 8) ?? source.fingerprint.slice(0, 12),
          ),
          source.dirty ? badge('modified source', 'warn') : null,
        ),
        el('p', { class: 'source-label mono' }, run.command.join(' ')),
      ),
      tabs,
    );
  }
}
