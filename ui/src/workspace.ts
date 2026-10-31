import { Investigations, type SourceSelection } from './investigation/state';
import { InvestigationHistory } from './investigation/history';
import type { View } from './investigation/view';
import { AnalysisWatch } from './analysis/watch';
import { AttemptMonitor } from './analysis/monitor';
import { attemptActivity } from './analysis/activity';
import {
  captureQuality,
  targetCheck,
  runStatusTone,
} from './recordings/presentation';
import { RecordingCatalog } from './recordings/catalog';
import { ImportCatalog } from './recordings/formats';
import { operation, type Run } from './api';
import {
  recordingDetails,
  recordingMetadata,
  queryFailureMessage,
} from './recordings/details';
import { badge, button, el, empty, errorView, loading } from './dom';
import { date } from './format';
import { icon } from './design/icons';
import { themeControl } from './design/theme';
import { recordDialog } from './recordings/create';
import { importRecordingDialog } from './recordings/import';
import { removeRecordingDialog } from './recordings/remove';
import { capabilities } from './views/capabilities';
import { overview } from './views/overview';
import { scenarios } from './views/scenarios';
import { projectStorage } from './projects/context';
import type { WorkspaceProject } from './projects/selection';

export class Workspace {
  private catalog = new RecordingCatalog();
  private importCatalog = new ImportCatalog();
  private selected = new URLSearchParams(location.search).get('run') ?? '';
  private tab =
    new URLSearchParams(location.search).get('view') ??
    (this.selected ? 'investigate' : 'recordings');
  private rail = el('div', { class: 'run-list', 'aria-label': 'Recordings' });
  private content = el('main', { class: 'workspace', id: 'main' });
  private epoch = 0;
  // User navigation owns foreground intent across automatic status rerenders.
  private navigationEpoch = 0;
  private query = '';
  private focusSelectedTab = false;
  private poll: ReturnType<typeof setTimeout> | undefined;
  private libraryPolling: AbortSignal | undefined;
  private libraryPollOffset = 0;
  private libraryStatusError: unknown;
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
    private project: WorkspaceProject,
  ) {
    let storage: ReturnType<typeof projectStorage> | undefined;
    try {
      storage = projectStorage(window.sessionStorage, project.storageScope);
    } catch {
      /* Private state remains in memory. */
    }
    this.investigations = new Investigations(storage);
    this.history = new InvestigationHistory(
      this.investigations,
      window,
      project.storageScope,
    );
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
      ...views,
      overview: {
        name: 'Details',
        render: (context) =>
          overview(
            context.run,
            context.summary,
            context.source,
            context.signal,
          ),
      },
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
      this.tab =
        params.get('view') ?? (this.selected ? 'investigate' : 'recordings');
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
      ![
        ...Object.keys(this.views),
        'recordings',
        'capabilities',
        'scenarios',
      ].includes(this.tab)
    )
      this.tab = this.selected ? 'investigate' : 'recordings';
  }

  mount(root: HTMLElement): void {
    const search = el('input', {
      type: 'search',
      class: 'run-search',
      placeholder: 'Find a recording',
      'aria-label': 'Find a recording',
    });
    search.addEventListener('input', () => {
      this.query = search.value;
      this.renderRail();
      if (this.tab === 'recordings') this.renderLibrary();
    });
    const refresh = button(
      '',
      () => {
        void this.reload();
      },
      'button subtle icon-button',
    );
    refresh.setAttribute('aria-label', 'Refresh recordings');
    refresh.setAttribute('title', 'Refresh recordings');
    refresh.append(icon('reset'));
    const library = button(
      'Recordings',
      () => this.navigate('', 'recordings'),
      'workspace-nav',
    );
    library.dataset.view = 'recordings';
    library.prepend(icon('recording'));
    const project = this.project.pickerHref
      ? el(
          'a',
          {
            class: 'workspace-project',
            href: this.project.pickerHref,
            'aria-label': `Switch project, current project ${this.project.name}`,
          },
          icon('folder'),
          el('span', {}, this.project.name),
          icon('down'),
        )
      : el(
          'div',
          { class: 'workspace-project', title: this.project.storePath },
          icon('folder'),
          el('span', {}, this.project.name || 'Local recordings'),
        );
    const sidebar = el(
      'aside',
      { class: 'sidebar' },
      el(
        'a',
        {
          class: 'wordmark',
          href: this.project.pickerHref ?? '#',
          'aria-label': 'Hunin projects',
        },
        'hunin',
      ),
      project,
      el(
        'nav',
        { class: 'workspace-navigation', 'aria-label': 'Workspace' },
        library,
      ),
      el(
        'div',
        { class: 'sidebar-title' },
        el('h2', {}, 'Recent recordings'),
        refresh,
      ),
      search,
      this.rail,
      el(
        'div',
        { class: 'sidebar-actions' },
        button('Record', () => this.openRecording(), 'button primary'),
        button('Import', () => this.openImport(), 'button subtle'),
      ),
      el(
        'nav',
        { class: 'workspace-tools', 'aria-label': 'Tools' },
        button(
          'Scenarios',
          () => this.navigate('', 'scenarios'),
          'text-button',
        ),
        button(
          'Runtime support',
          () => this.navigate('', 'capabilities'),
          'text-button',
        ),
        themeControl(),
      ),
    );
    root.replaceChildren(
      el('a', { class: 'skip-link', href: '#main' }, 'Skip to recordings'),
      el('div', { class: 'app-shell' }, sidebar, this.content),
    );
    this.rail.replaceChildren(loading('Loading recordings…'));
    void this.reload();
  }

  private renderLibrary(): void {
    const focused = this.content.querySelector<HTMLButtonElement>(
      '.recording-card:focus',
    )?.dataset.run;
    const scroll = this.content.scrollTop;
    const shown = this.catalog.runs.filter((run) =>
      `${run.name} ${run.runtime} ${run.id}`
        .toLowerCase()
        .includes(this.query.toLowerCase()),
    );
    const rows = el('div', {
      class: 'recording-library',
      'aria-label': 'Recordings',
    });
    for (const run of shown) {
      const row = button(
        '',
        () => this.navigate(run.id, 'investigate'),
        'recording-card',
      );
      row.dataset.run = run.id;
      row.append(
        icon('recording'),
        el(
          'div',
          { class: 'recording-card-title' },
          el('strong', {}, run.name),
          el('span', {}, run.runtime),
        ),
        el(
          'time',
          { datetime: new Date(run.started_at_us / 1000).toISOString() },
          date(new Date(run.started_at_us / 1000).toISOString()),
        ),
        badge(run.status, runStatusTone(run.status)),
        icon('chevron-right'),
      );
      rows.append(row);
    }
    const title = el(
      'div',
      { class: 'library-heading' },
      el(
        'div',
        {},
        el('p', { class: 'eyebrow' }, this.project.name),
        el('h1', {}, 'Recordings'),
        this.catalog.hasOlder
          ? el(
              'p',
              { class: 'muted library-scope' },
              `${this.catalog.runs.length} recordings loaded. Load older recordings to search more.`,
            )
          : null,
      ),
      el(
        'div',
        { class: 'toolbar' },
        button('Import', () => this.openImport(), 'button subtle'),
        button('Record', () => this.openRecording(), 'button primary'),
      ),
    );
    this.content.replaceChildren(
      title,
      shown.length
        ? rows
        : this.catalog.runs.length
          ? empty(
              'No matching recordings',
              `No recording matches “${this.query}”.`,
              button('Clear search', () => {
                this.query = '';
                const search =
                  document.querySelector<HTMLInputElement>('.run-search');
                if (search) search.value = '';
                this.renderRail();
                this.renderLibrary();
              }),
            )
          : empty(
              'No recordings yet',
              'Record a command or import a profile.',
              button('Record', () => this.openRecording(), 'button primary'),
            ),
    );
    if (this.catalog.hasOlder) {
      const more = button(
        'Load older recordings',
        () => this.loadOlderRecordings(more),
        'button subtle library-more',
      );
      more.disabled = this.catalog.loadingOlder;
      this.content.append(more);
    }
    if (this.libraryStatusError)
      this.content.append(
        errorView(this.libraryStatusError, () => {
          this.libraryStatusError = undefined;
          void this.reload(undefined, true);
        }),
      );
    if (focused)
      this.content
        .querySelector<HTMLButtonElement>(
          `.recording-card[data-run="${CSS.escape(focused)}"]`,
        )
        ?.focus({ preventScroll: true });
    this.content.scrollTop = scroll;
    document.title = `${this.project.name} · Recordings · Hunin`;
    this.pollLibrary();
  }

  private pollLibrary(): void {
    clearTimeout(this.poll);
    const signal = this.renderController?.signal;
    if (
      this.tab !== 'recordings' ||
      !signal ||
      signal.aborted ||
      this.libraryPolling === signal
    )
      return;
    const running = this.catalog.runs.filter((run) => run.status === 'running');
    if (!running.length) return;
    const epoch = this.epoch;
    const selected = Array.from(
      { length: Math.min(8, running.length) },
      (_, index) => running[(this.libraryPollOffset + index) % running.length],
    );
    this.libraryPollOffset =
      (this.libraryPollOffset + selected.length) % running.length;
    this.poll = setTimeout(() => {
      this.libraryPolling = signal;
      void (async () => {
        let changed = Boolean(this.libraryStatusError);
        for (const previous of selected) {
          const { run } = await recordingMetadata(previous.id, signal);
          if (
            epoch !== this.epoch ||
            signal.aborted ||
            this.tab !== 'recordings'
          )
            return;
          changed ||=
            previous.status !== run.status ||
            previous.finished_at_us !== run.finished_at_us;
          this.catalog.observe(run);
        }
        this.libraryStatusError = undefined;
        if (changed) {
          this.renderRail();
          this.renderLibrary();
        }
      })()
        .catch((error) => {
          if (
            epoch === this.epoch &&
            !signal.aborted &&
            this.tab === 'recordings'
          ) {
            this.libraryStatusError = error;
            this.renderLibrary();
          }
        })
        .finally(() => {
          if (this.libraryPolling === signal) this.libraryPolling = undefined;
          if (
            epoch === this.epoch &&
            !signal.aborted &&
            this.tab === 'recordings'
          )
            this.pollLibrary();
        });
    }, 3000);
  }

  private openRecording(): void {
    const navigation = this.navigationEpoch;
    recordDialog({
      recorded: (id, foreground) =>
        this.recordingAccepted(
          id,
          foreground && navigation === this.navigationEpoch,
        ),
      unconfirmed: () => {
        void this.reload(undefined, true);
      },
    });
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
          this.tab = 'recordings';
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
        if (this.tab === 'recordings') this.renderLibrary();
        return;
      }
      if (select) {
        this.selected = select;
        this.tab = 'investigate';
        this.writeLocation();
      }
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
    for (const control of document.querySelectorAll<HTMLButtonElement>(
      '.workspace-navigation [data-view]',
    ))
      control.setAttribute(
        'aria-current',
        control.dataset.view === this.tab ? 'page' : 'false',
      );
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
            ['recordings', 'capabilities', 'scenarios'].includes(this.tab)
              ? 'investigate'
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
      const more = button(
        'Load older recordings',
        () => this.loadOlderRecordings(more),
        'button subtle small',
      );
      more.disabled = this.catalog.loadingOlder;
      this.rail.append(more);
    }
  }

  private loadOlderRecordings(trigger: HTMLButtonElement): void {
    if (trigger.disabled) return;
    trigger.disabled = true;
    const navigation = this.navigationEpoch;
    const hadFocus = document.activeElement === trigger;
    void this.catalog
      .older()
      .then((read) => {
        if (!read.current()) return;
        if (read.error) {
          if (navigation === this.navigationEpoch)
            (this.tab === 'recordings' ? this.content : this.rail).append(
              errorView(read.error, () => {
                void this.reload();
              }),
            );
          return;
        }
        this.renderRail();
        if (this.tab === 'recordings') {
          this.renderLibrary();
          if (hadFocus && navigation === this.navigationEpoch)
            (
              this.content.querySelector<HTMLButtonElement>('.library-more') ??
              this.content.querySelector<HTMLButtonElement>(
                '.recording-card:last-child',
              )
            )?.focus();
        }
      })
      .finally(() => {
        trigger.disabled = false;
      });
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
    this.content.dataset.view = this.tab;
    this.renderController?.abort();
    const controller = new AbortController();
    this.renderController = controller;
    const signal = controller.signal;
    clearTimeout(this.poll);
    this.attemptMonitor.select(this.selected);
    this.content.replaceChildren(loading('Loading recording…'));
    try {
      if (this.tab === 'recordings') {
        this.renderLibrary();
        return;
      }
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
            'No recording selected',
            'Choose a recording from the sidebar.',
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
                ? 'Results changed. Select an observation again; your filters are kept.'
                : 'Saved selections could not be verified. Your filters are kept.',
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
            'Selections are saved on this page only. Browser storage is unavailable.',
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
          `Back to ${this.views[this.tab]?.name ?? 'recording'}`,
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
        const renderer = this.views[this.tab] ?? this.views.investigate;
        try {
          const body = await renderer.render({
            run,
            summary,
            runs: this.catalog.runs,
            importFormats: (signal) => this.importCatalog.read(signal),
            source,
            investigation,
            signal,
            persist: (mode = 'replace') => {
              if (!signal.aborted) this.writeLocation(mode);
            },
            countFunctions: (functions) => {
              if (signal.aborted) return;
              investigation.calls.functions = functions.map((value) => ({
                ...value,
              }));
              this.investigations.persist(run.id);
              this.navigate(run.id, 'calls');
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
    const quality = captureQuality(run);
    const target = targetCheck(run);
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
            const navigation = this.navigationEpoch;
            const current = () =>
              this.selected === run.id && navigation === this.navigationEpoch;
            void operation('runs.cancel', { run_id: run.id })
              .then(() => {
                this.messages.set(run.id, 'Stop requested.');
                void this.reload(undefined, !current());
              })
              .catch((error) => {
                if (current())
                  this.content.prepend(
                    errorView(error, () => {
                      void this.reload();
                    }),
                  );
                else {
                  this.messages.set(
                    run.id,
                    'The stop request could not be confirmed. Check the recording status.',
                  );
                  void this.reload(undefined, true);
                }
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
        el(
          'div',
          { class: 'run-heading' },
          el(
            'div',
            {},
            el('p', { class: 'eyebrow' }, this.project.name),
            el('h1', {}, run.name || run.id.slice(0, 12)),
          ),
          actions,
        ),
        el(
          'div',
          { class: 'run-meta' },
          el('span', {}, run.runtime),
          el(
            'time',
            { datetime: new Date(run.started_at_us / 1000).toISOString() },
            date(new Date(run.started_at_us / 1000).toISOString()),
          ),
          el(
            'span',
            { class: `badge ${quality.tone}`, title: quality.detail },
            quality.label,
          ),
          el(
            'span',
            { class: `badge ${target.tone}`, title: target.detail },
            target.label,
          ),
        ),
        quality.tone === 'warn' || quality.tone === 'bad'
          ? el('p', { class: 'capture-notice' }, quality.detail)
          : null,
      ),
      tabs,
    );
  }
}
