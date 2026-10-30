import { Investigations, type SourceSelection } from './investigation/state';
import { InvestigationHistory } from './investigation/history';
import type { View } from './investigation/view';
import { AnalysisWatch } from './analysis/watch';
import { AttemptMonitor } from './analysis/monitor';
import { attemptActivity } from './analysis/activity';
import { runSource, runStatusTone } from './recordings/presentation';
import { RecordingCatalog } from './recordings/catalog';
import { operation, type Run } from './api';
import {
  recordingDetails,
  recordingMetadata,
  queryFailureMessage,
} from './recordings/details';
import { badge, button, el, empty, errorView, loading } from './dom';
import { date } from './format';
import { recordDialog } from './recordings/create';
import { importRecordingDialog } from './recordings/import';
import { removeRecordingDialog } from './recordings/remove';
import { capabilities } from './views/capabilities';
import { overview } from './views/overview';
import { scenarios } from './views/scenarios';

export class Workspace {
  private catalog = new RecordingCatalog();
  private selected = new URLSearchParams(location.search).get('run') ?? '';
  private tab = new URLSearchParams(location.search).get('view') ?? 'overview';
  private rail = el('div', { class: 'run-list', 'aria-label': 'Recordings' });
  private content = el('main', { class: 'workspace', id: 'main' });
  private epoch = 0;
  // User navigation owns foreground intent across automatic status rerenders.
  private navigationEpoch = 0;
  private query = '';
  private focusSelectedTab = false;
  private poll: ReturnType<typeof setTimeout> | undefined;
  private views: Record<string, View>;
  private investigations: Investigations;
  private history: InvestigationHistory;
  private renderController: AbortController | undefined;
  private pendingInvalidation: 'changed' | 'unconfirmed' | undefined;
  private returningFromSource = false;
  private messages = new Map<string, string>();
  private analysisWatch: AnalysisWatch;
  private attemptMonitor: AttemptMonitor;

  constructor(
    views: Record<string, View>,
    private sourceView: (
      run: Run,
      selected: SourceSelection,
      signal: AbortSignal,
      page: (startLine: number) => void,
    ) => Promise<HTMLElement>,
  ) {
    let storage: Storage | undefined;
    try {
      storage = window.sessionStorage;
    } catch {
      /* Private state remains in memory. */
    }
    this.investigations = new Investigations(storage);
    this.history = new InvestigationHistory(this.investigations);
    this.analysisWatch = new AnalysisWatch(storage);
    this.attemptMonitor = new AttemptMonitor(this.analysisWatch, {
      updated: (runId) => this.updateAttemptNotice(runId),
      completed: (runId, attempts) => {
        this.messages.set(
          runId,
          attempts
            .map(
              (attempt) => `Analyzer attempt ${attempt.id}: ${attempt.status}.`,
            )
            .join(' '),
        );
        if (this.selected === runId) void this.render();
      },
    });
    this.views = {
      overview: {
        name: 'Overview',
        render: (context) =>
          overview(
            context.run,
            context.summary,
            context.source,
            context.signal,
          ),
      },
      ...views,
    };
    this.normalizeTab();
    this.history.restore(this.selected);
    window.addEventListener('pagehide', () => {
      this.attemptMonitor.pause();
      this.renderController?.abort();
      this.catalog.pause();
      clearTimeout(this.poll);
    });
    window.addEventListener('pageshow', (event) => {
      this.attemptMonitor.select(this.selected);
      if (event.persisted) void this.reload();
    });
    window.addEventListener('online', () => this.attemptMonitor.recheck());
    window.addEventListener('popstate', () => {
      this.navigationEpoch++;
      const wasSource = Boolean(
        this.selected && this.investigations.forRun(this.selected).source,
      );
      const params = new URLSearchParams(location.search);
      this.selected = params.get('run') ?? '';
      this.tab = params.get('view') ?? 'overview';
      this.normalizeTab();
      this.history.restore(this.selected);
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
          button('Import', () => this.openImport(), 'button subtle'),
          button('+ Record', () => this.openRecording(), 'button primary'),
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

  private openRecording(): void {
    const navigation = this.navigationEpoch;
    recordDialog((id, foreground) =>
      this.recordingAccepted(
        id,
        foreground && navigation === this.navigationEpoch,
      ),
    );
  }

  private recordingAccepted(id: string, foreground: boolean): void {
    if (foreground) {
      this.navigationEpoch++;
      this.focusSelectedTab = true;
      void this.reload(id);
    } else void this.reload(undefined, true);
  }

  private openImport(): void {
    const navigation = this.navigationEpoch;
    importRecordingDialog({
      imported: (id, foreground) =>
        this.recordingAccepted(
          id,
          foreground && navigation === this.navigationEpoch,
        ),
      unconfirmed: () => {
        void this.reload(undefined, true);
      },
    });
  }

  private removeRecording(run: Run): void {
    removeRecordingDialog(run, {
      removed: () => {
        this.investigations.forget(run.id);
        this.analysisWatch.forgetRun(run.id);
        this.messages.delete(run.id);
        if (this.selected === run.id) {
          this.renderController?.abort();
          this.attemptMonitor.pause();
          this.selected = '';
          this.tab = 'overview';
          this.writeLocation('replace');
          void this.reload();
        } else void this.reload(undefined, true);
      },
      unconfirmed: () => {
        void this.reload(undefined, this.selected !== run.id);
      },
    });
  }

  private async reload(select?: string, preserveView = false): Promise<void> {
    const navigation = this.navigationEpoch;
    const read = await this.catalog.refresh(() =>
      navigation === this.navigationEpoch
        ? (select ?? this.selected)
        : this.selected,
    );
    if (!read.current()) return;
    try {
      if ('error' in read) throw read.error;
      if (preserveView || navigation !== this.navigationEpoch) {
        this.renderRail();
        return;
      }
      if (select) {
        this.selected = select;
        this.tab = 'overview';
        this.writeLocation();
      }
      if (!this.selected && !['capabilities', 'scenarios'].includes(this.tab))
        this.selected = this.catalog.runs[0]?.id ?? '';
      this.renderRail();
      await this.render();
    } catch (error) {
      if (read.current() && navigation === this.navigationEpoch) {
        const feedback = errorView(error, () => {
          void this.reload(undefined, preserveView);
        });
        if (preserveView) this.rail.append(feedback);
        else this.content.replaceChildren(feedback);
      }
    }
  }

  private renderRail(): void {
    this.rail.replaceChildren();
    const shown = this.catalog.runs.filter((run) =>
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
          badge(run.status, runStatusTone(run.status)),
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
          this.catalog.runs.length
            ? 'No matching recordings.'
            : 'No recordings yet. Record a workload to begin.',
        ),
      );
    if (this.catalog.hasOlder) {
      const older = button(
        'Load older recordings',
        () => {
          older.disabled = true;
          void this.catalog
            .older()
            .then((read) => {
              if (!read.current()) return;
              if ('error' in read)
                this.rail.append(
                  errorView(read.error, () => {
                    void this.reload();
                  }),
                );
              else this.renderRail();
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

  private writeLocation(
    mode: 'push' | 'replace' = 'replace',
    sourceEntry = 0,
  ): void {
    if (mode === 'push') this.navigationEpoch++;
    this.history.write(
      { runId: this.selected, view: this.tab },
      mode,
      sourceEntry,
    );
  }

  private async render(): Promise<void> {
    const epoch = ++this.epoch;
    this.renderController?.abort();
    const controller = new AbortController();
    this.renderController = controller;
    const signal = controller.signal;
    clearTimeout(this.poll);
    this.attemptMonitor.select(this.selected);
    this.content.replaceChildren(loading('Loading recording…'));
    try {
      if (this.tab === 'capabilities') {
        const view = await capabilities(signal);
        if (epoch === this.epoch) {
          this.content.replaceChildren(el('div', { class: 'view' }, view));
          document.title = 'Capabilities · Hunin';
        }
        return;
      }
      if (this.tab === 'scenarios') {
        const view = await scenarios((id) => {
          this.recordingAccepted(id, !signal.aborted);
        }, signal);
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
              () => this.openRecording(),
              'button primary',
            ),
          ),
        );
        return;
      }
      const details = await recordingDetails(this.selected, signal);
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
      if (this.catalog.ensure(run)) this.renderRail();
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
      if (!summary)
        notices.push(
          el(
            'p',
            { class: 'notice', role: 'alert' },
            `Recording aggregate unavailable: ${queryFailureMessage(details.summaryError)}. Counts remain unknown; other recording views query their own data.`,
          ),
        );
      if (details.result_identity_diagnostic)
        notices.push(
          el(
            'p',
            { class: 'notice', role: 'status' },
            `Selected result identity is unconfirmed: ${details.result_identity_diagnostic}`,
          ),
        );
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
      this.content.replaceChildren(
        this.header(run),
        ...notices,
        el('div', { 'data-analysis-activity': run.id }),
        view,
      );
      this.updateAttemptNotice(run.id);
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
            if (!this.history.returnToView(run.id)) {
              this.navigationEpoch++;
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
            selected,
            signal,
            (startLine) => {
              if (signal.aborted) return;
              const depth = this.history.state()?.sourceReturn ?? 0;
              investigation.source = { ...selected, startLine };
              this.writeLocation('push', depth ? depth + 1 : 0);
              void this.render();
            },
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
        try {
          const body = await renderer.render({
            run,
            summary,
            runs: this.catalog.runs,
            source,
            investigation,
            signal,
            persist: (mode = 'replace') => {
              if (!signal.aborted) this.writeLocation(mode);
            },
            followAttempt: (id) => {
              this.attemptMonitor.follow(run.id, id);
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
        } catch (error) {
          if (epoch === this.epoch && !signal.aborted)
            view.replaceChildren(
              errorView(error, () => {
                void this.render();
              }),
            );
        }
      }
      if (epoch !== this.epoch || signal.aborted) return;
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

  private updateAttemptNotice(runId: string): void {
    const target = this.content.querySelector('[data-analysis-activity]');
    if (target?.getAttribute('data-analysis-activity') !== runId) return;
    const notice = attemptActivity(this.analysisWatch, runId, {
      recheck: () => this.attemptMonitor.recheck(),
      open: () => this.navigate(runId, 'analyses'),
    });
    target.replaceChildren(...(notice ? [notice] : []));
  }

  private pollStatus(id: string, epoch: number, signal: AbortSignal): void {
    this.poll = setTimeout(() => {
      void recordingMetadata(id, signal)
        .then((details) => {
          if (epoch !== this.epoch || signal.aborted) return;
          const { run, result_identity } = details;
          this.catalog.observe(run);
          this.renderRail();
          const invalidation = this.investigations.observeResult(
            id,
            result_identity,
          );
          if (invalidation) {
            this.pendingInvalidation = invalidation;
            void this.render();
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
    this.writeLocation('push', 1);
    void this.render();
  }

  private header(run: Run): HTMLElement {
    const source = runSource(run);
    const actions = el('div', { class: 'toolbar' });
    const remove = button(
      'Remove recording',
      () => this.removeRecording(run),
      'button danger small',
    );
    remove.disabled = run.status === 'running';
    if (remove.disabled)
      remove.title = 'Stop the recording before removing it.';
    actions.append(remove);
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
          badge(run.status, runStatusTone(run.status)),
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
