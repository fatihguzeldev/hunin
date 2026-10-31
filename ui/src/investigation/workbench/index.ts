import { operation } from '../../api';
import { button, el, empty } from '../../dom';
import { icon } from '../../design/icons';
import { numeric } from '../../format';
import { weight } from '../../profile-weight';
import { linkedRequest } from '../../requests';
import { hasInspectableSource } from '../../source/paths';
import type { Investigation } from '../state';
import type { ViewContext } from '../view';
import { activity } from './activity';
import { functions } from './functions';
import { inspector } from './inspector';
import { framePath, nodeAt, type Inspection } from './model';
import { clampWindow, type TimeWindow } from './selection';
import { stacks } from './stacks';
import './workbench.css';

type Selection = Pick<
  Investigation['workbench'],
  'unit' | 'window' | 'selectedFrameId' | 'clockDomain'
>;

export async function investigate(context: ViewContext): Promise<HTMLElement> {
  const state = context.investigation.workbench;
  const root = el('div', { class: 'workbench' });
  const feedback = el('div', {
    class: 'wb-feedback',
    role: 'status',
    'aria-live': 'polite',
  });
  let data: Inspection;
  let request: AbortController | undefined;
  let render: AbortController | undefined;
  let requestEpoch = 0;
  let acceptedEpoch = 0;
  let interactionEpoch = 0;
  const interacted = () => {
    interactionEpoch++;
  };
  document.addEventListener('pointerdown', interacted, {
    signal: context.signal,
  });
  document.addEventListener('keydown', interacted, { signal: context.signal });
  document.addEventListener('wheel', interacted, {
    signal: context.signal,
    passive: true,
  });
  let showInspector = !matchMedia('(max-width: 1050px)').matches;
  let accepted: Selection | undefined;
  let selectedPath: string[] | undefined;
  const selection = (): Selection => ({
    unit: state.unit,
    clockDomain: state.clockDomain,
    selectedFrameId: state.selectedFrameId,
    window: state.window ? { ...state.window } : undefined,
  });

  async function read(signal: AbortSignal): Promise<Inspection> {
    return operation(
      'profiles.inspect',
      {
        run_id: context.run.id,
        weight_unit: state.unit || undefined,
        clock_domain:
          state.clockDomain === 'unix_epoch' ? undefined : state.clockDomain,
        start_us: state.window?.start_us,
        end_us: state.window?.end_us,
        frame_id: state.selectedFrameId,
        bins: 120,
        function_limit: 1000,
        node_limit: 3000,
        depth_limit: 128,
      },
      signal,
    );
  }

  async function load(initial = false): Promise<void> {
    if (context.signal.aborted) return;
    const epoch = ++requestEpoch;
    request?.abort();
    request = new AbortController();
    const linked = linkedRequest(context.signal, request.signal);
    const intent = selection();
    root.setAttribute('aria-busy', 'true');
    feedback.replaceChildren(el('span', {}, 'Updating window…'));
    try {
      let next = await read(linked.signal);
      if (linked.signal.aborted || epoch !== requestEpoch) return;
      if (
        initial &&
        !state.clockDomain &&
        !next.bounds &&
        next.clock_domains.length
      ) {
        state.clockDomain = next.clock_domains[0];
        state.window = undefined;
        state.selectedFrameId = undefined;
        state.focusPath = [];
        next = await read(linked.signal);
      }
      if (initial && state.window && next.bounds) {
        const bounded = clampWindow(state.window, next.bounds);
        if (
          bounded.start_us !== state.window.start_us ||
          bounded.end_us !== state.window.end_us
        ) {
          state.window = bounded;
          next = await read(linked.signal);
        }
      }
      if (linked.signal.aborted || epoch !== requestEpoch) return;
      if (!next.selected && next.functions.length) {
        state.selectedFrameId = next.functions[0].id;
        state.inspectorTab = hasInspectableSource(
          next.functions[0].file,
          next.functions[0].code_id,
        )
          ? 'source'
          : 'callers';
        next = await read(linked.signal);
      }
      if (linked.signal.aborted || epoch !== requestEpoch) return;
      data = next;
      acceptedEpoch = epoch;
      accepted = selection();
      feedback.setAttribute('role', 'status');
      feedback.replaceChildren();
      context.persist('replace');
      draw();
    } catch (error) {
      if (linked.signal.aborted || epoch !== requestEpoch) return;
      if (initial) throw error;
      if (accepted) Object.assign(state, accepted);
      context.persist('replace');
      draw();
      feedback.replaceChildren(
        el('span', {}, error instanceof Error ? error.message : String(error)),
        button(
          'Try again',
          () => {
            Object.assign(state, intent);
            void load();
          },
          'button subtle',
        ),
      );
      feedback.setAttribute('role', 'alert');
    } finally {
      linked.finish();
      if (epoch === requestEpoch) root.removeAttribute('aria-busy');
    }
  }

  function changeWindow(window?: TimeWindow) {
    selectedPath = undefined;
    state.window = window;
    state.focusPath = [];
    context.persist('push');
    void load();
  }
  function select(id: string, path?: string[]) {
    if (context.signal.aborted) return;
    state.selectedFrameId = id;
    selectedPath = path;
    const frame = data.functions.find((frame) => frame.id === id);
    state.inspectorTab =
      frame && hasInspectableSource(frame.file, frame.code_id)
        ? 'source'
        : 'callers';
    showInspector = true;
    context.persist('push');
    const focusIntent = interactionEpoch;
    const pending = load();
    const selectionRequest = requestEpoch;
    void pending.then(() => {
      if (
        !context.signal.aborted &&
        requestEpoch === selectionRequest &&
        acceptedEpoch === selectionRequest &&
        interactionEpoch === focusIntent &&
        state.selectedFrameId === id &&
        matchMedia('(max-width: 1050px)').matches
      )
        root.querySelector<HTMLElement>('.wb-inspector-name')?.focus();
    });
  }
  function focus() {
    if (!data.selected) return;
    const path =
      selectedPath &&
      nodeAt(data.roots, selectedPath)?.frame_id === data.selected.id
        ? selectedPath
        : framePath(data.roots, data.selected.id);
    if (path) {
      state.focusPath = path;
      state.mode = 'stacks';
      context.persist('push');
      draw();
    } else {
      feedback.replaceChildren(
        el(
          'span',
          {},
          data.selected.sample_count
            ? 'This call path is outside the current tree limit.'
            : 'No samples for this function in this window.',
        ),
        button('Locate in time', locate, 'button subtle'),
      );
    }
  }
  function locate() {
    if (!data.selected || !data.bounds) return;
    changeWindow(
      clampWindow(
        {
          start_us: data.selected.observed_start_us,
          end_us: Math.max(
            data.selected.observed_start_us + 1,
            data.selected.observed_end_us,
          ),
        },
        data.bounds,
      ),
    );
  }
  function draw() {
    render?.abort();
    render = new AbortController();
    const lifetime = linkedRequest(context.signal, render.signal);
    render.signal.addEventListener('abort', lifetime.finish, { once: true });
    const active =
      document.activeElement instanceof HTMLElement &&
      root.contains(document.activeElement)
        ? document.activeElement
        : undefined;
    const activeFrame = active?.dataset.wbFrame;
    const activePath = active?.dataset.wbPath;
    const activeControl = active?.dataset.wbControl;
    const activeId = active?.id;
    const activeLabel = active?.getAttribute('aria-label');
    const scroll =
      root.querySelector('.wb-functions-scroll, .wb-stacks')?.scrollTop ?? 0;
    root.classList.toggle(
      'wb-wide-inspector',
      state.inspectorWide && showInspector && Boolean(data.bounds),
    );
    root.classList.toggle(
      'wb-details-open',
      showInspector && Boolean(data.bounds),
    );
    const units = el('select', {
      'aria-label': 'Sample weight unit',
      class: 'wb-unit',
    });
    const options = data.units.includes(data.weight_unit)
      ? data.units
      : [data.weight_unit, ...data.units];
    for (const unit of options)
      units.append(el('option', { value: unit }, unit));
    units.value = data.weight_unit;
    units.addEventListener('change', () => {
      state.unit = units.value;
      state.window = undefined;
      state.selectedFrameId = undefined;
      state.focusPath = [];
      selectedPath = undefined;
      context.persist('push');
      void load();
    });
    const clocks = el(
      'select',
      { 'aria-label': 'Clock domain', class: 'wb-unit' },
      el('option', { value: 'unix_epoch' }, 'Aligned epoch'),
    );
    const domains = new Set([
      ...data.clock_domains,
      ...(data.clock_domain ? [data.clock_domain] : []),
    ]);
    for (const [index, domain] of [...domains].entries())
      clocks.append(
        el(
          'option',
          { value: domain, title: domain },
          `Runtime clock ${index + 1}`,
        ),
      );
    clocks.value = data.clock_domain ?? 'unix_epoch';
    if (data.clock_domain) clocks.title = data.clock_domain;
    clocks.addEventListener('change', () => {
      state.clockDomain = clocks.value;
      state.window = undefined;
      state.selectedFrameId = undefined;
      state.focusPath = [];
      selectedPath = undefined;
      context.persist('push');
      void load();
    });
    const title = el(
      'div',
      { class: 'wb-scope-bar' },
      el(
        'span',
        {},
        'Sampled weight ',
        el(
          'strong',
          { class: 'mono' },
          weight(data.total_weight, data.weight_unit),
        ),
      ),
      units,
      clocks,
    );
    const body = el('div', { class: 'wb-body' });
    const analysis = el('section', {
      class: 'wb-analysis',
      'aria-label': 'Selected-window analysis',
    });
    const tabs = el('div', {
      role: 'tablist',
      'aria-label': 'Analysis view',
      class: 'wb-analysis-tabs',
    });
    for (const [mode, label, drawing] of [
      ['functions', 'Functions', 'code'],
      ['stacks', 'Stacks', 'stacks'],
    ] as const) {
      const tab = button(
        label,
        () => {
          state.mode = mode;
          context.persist();
          draw();
          root
            .querySelector<HTMLButtonElement>(`[data-wb-mode="${mode}"]`)
            ?.focus();
        },
        'wb-analysis-tab',
      );
      tab.prepend(icon(drawing));
      tab.setAttribute('role', 'tab');
      tab.setAttribute('aria-selected', String(state.mode === mode));
      tab.setAttribute('data-wb-mode', mode);
      tab.tabIndex = state.mode === mode ? 0 : -1;
      tabs.append(tab);
    }
    tabs.addEventListener('keydown', (event) => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key))
        return;
      event.preventDefault();
      state.mode =
        event.key === 'Home'
          ? 'functions'
          : event.key === 'End'
            ? 'stacks'
            : state.mode === 'functions'
              ? 'stacks'
              : 'functions';
      context.persist();
      draw();
      root
        .querySelector<HTMLButtonElement>(`[data-wb-mode="${state.mode}"]`)
        ?.focus();
    });
    const detailToggle = button(
      showInspector ? 'Hide details' : 'Function details',
      () => {
        showInspector = !showInspector;
        draw();
      },
      'button subtle wb-details-toggle',
    );
    detailToggle.setAttribute('aria-expanded', String(showInspector));
    detailToggle.setAttribute('data-wb-control', 'details');
    const results =
      state.mode === 'functions'
        ? functions(data, state, select, () => context.persist())
        : stacks(data, state, select, () => context.persist());
    analysis.append(
      el('div', { class: 'wb-analysis-toolbar' }, tabs, detailToggle),
      results,
    );
    body.append(analysis);
    if (showInspector)
      body.append(
        inspector(
          context,
          data,
          {
            select,
            focus,
            locate,
            redraw: draw,
            close: () => {
              showInspector = false;
              draw();
            },
          },
          lifetime.signal,
        ),
      );
    const scope = el(
      'details',
      { class: 'wb-query-scope' },
      el('summary', {}, 'Query scope'),
      el(
        'p',
        {},
        `${numeric(data.total_sample_count)} samples in ${data.clock_domain ?? 'the aligned epoch clock'} · ${numeric(data.unaligned_count)} samples outside this clock domain`,
      ),
    );
    for (const diagnostic of data.diagnostics)
      scope.append(el('p', {}, diagnostic));
    if (!data.bounds) {
      const url = new URL(location.href);
      url.searchParams.set('view', 'profile');
      body.prepend(
        empty(
          data.unaligned_count
            ? 'No samples in this clock domain'
            : 'No stack samples',
          data.unaligned_count
            ? 'The call tree can still show their sampled weights.'
            : context.run.status === 'running'
              ? 'Samples will appear as this recording is collected.'
              : 'This recording has no sampled stacks for this unit.',
        ),
      );
      const emptyState = body.firstElementChild;
      body.replaceChildren(...(emptyState ? [emptyState] : []));
      if (data.unaligned_count)
        body.append(
          el('a', { class: 'button', href: url.href }, 'Open call tree'),
        );
    }
    const children = [
      feedback,
      title,
      data.bounds ? activity(data, changeWindow, lifetime.signal) : null,
      body,
      data.unaligned_count
        ? el(
            'p',
            { class: 'wb-scope-notice' },
            `${numeric(data.unaligned_count)} samples are outside the selected clock domain and are not included in this timeline.`,
          )
        : null,
      scope,
    ];
    root.replaceChildren(
      ...children.filter((node): node is HTMLElement => node !== null),
    );
    const pane = root.querySelector('.wb-functions-scroll, .wb-stacks');
    if (pane) pane.scrollTop = scroll;
    const focusSelector = activePath
      ? `[data-wb-path="${CSS.escape(activePath)}"]`
      : activeFrame
        ? `[data-wb-frame="${CSS.escape(activeFrame)}"]`
        : activeControl
          ? `[data-wb-control="${CSS.escape(activeControl)}"]`
          : activeId
            ? `#${CSS.escape(activeId)}`
            : activeLabel
              ? `[aria-label="${CSS.escape(activeLabel)}"]`
              : undefined;
    if (focusSelector) {
      const restored = root.querySelector<HTMLElement>(focusSelector);
      if (restored) restored.focus({ preventScroll: true });
      else if (activeLabel === 'Close details')
        detailToggle.focus({ preventScroll: true });
    }
  }
  await load(true);
  return root;
}
