import { operation } from '../../api';
import { badge, button, el } from '../../dom';
import { icon } from '../../design/icons';
import { numeric } from '../../format';
import { weight, weightShare } from '../../profile-weight';
import { hasInspectableSource } from '../../source/paths';
import { sourcePresentation } from '../../source/presentation';
import { sourceUnavailable } from '../../source/errors';
import { linkedLifetime } from '../../requests';
import { mappedOrigins } from '../../code/view';
import type { ViewContext } from '../view';
import {
  shortPath,
  timeLabel,
  type Inspection,
  type SelectedFunction,
} from './model';

type Actions = {
  select: (id: string) => void;
  focus: () => void;
  locate: () => void;
  redraw: () => void;
  close: () => void;
};

export function inspector(
  context: ViewContext,
  data: Inspection,
  actions: Actions,
  signal: AbortSignal,
): HTMLElement {
  const selected = data.selected;
  const state = context.investigation.workbench;
  const root = el('aside', {
    class: 'wb-inspector',
    'aria-label': selected
      ? `${selected.function} details`
      : 'Function details',
  });
  if (!selected) {
    root.append(
      el('h2', {}, 'Function details'),
      el('p', {}, 'Select a function or a stack frame.'),
    );
    return root;
  }
  const tools = el('div', { class: 'wb-inspector-tools' });
  for (const [name, drawing, action] of [
    [
      state.inspectorWide ? 'Narrow details panel' : 'Widen details panel',
      'expand',
      () => {
        state.inspectorWide = !state.inspectorWide;
        context.persist();
        actions.redraw();
      },
    ],
    ['Close details', 'close', actions.close],
  ] as const) {
    const control = button('', action, 'button subtle wb-icon-button');
    control.append(icon(drawing));
    control.setAttribute('aria-label', name);
    control.title = name;
    tools.append(control);
  }
  const heading = el(
    'h2',
    { class: 'wb-inspector-name mono', tabindex: -1 },
    selected.function,
  );
  root.append(
    el(
      'div',
      { class: 'wb-inspector-top' },
      el('span', { class: 'wb-eyebrow' }, 'Selected function'),
      tools,
    ),
    heading,
  );
  const location = el(
    'details',
    { class: 'wb-location' },
    el(
      'summary',
      {},
      selected.file
        ? `${shortPath(selected.file)}${selected.line ? `:${selected.line}` : ''}`
        : selected.module || 'No source location',
    ),
  );
  if (selected.file) location.append(el('p', { class: 'mono' }, selected.file));
  root.append(location);
  if (data.window && data.bounds)
    root.append(
      el(
        'p',
        { class: 'wb-inspector-window' },
        'Time window ',
        el(
          'span',
          { class: 'mono' },
          timeLabel(data.window, data.bounds.start_us),
        ),
      ),
    );
  const stats = el('div', { class: 'wb-inspector-stats' });
  for (const [label, value] of [
    ['Self', selected.self_weight],
    ['Inclusive', selected.inclusive_weight],
  ] as const) {
    const share = weightShare(value, data.total_weight);
    stats.append(
      el(
        'div',
        {},
        el('span', {}, label),
        el(
          'strong',
          { class: 'mono' },
          share === undefined ? '—' : `${numeric(share)}%`,
        ),
        el('span', { class: 'mono' }, weight(value, data.weight_unit)),
      ),
    );
  }
  const focus = button('Focus call path', actions.focus, 'button primary');
  focus.prepend(icon('focus'));
  const locate = button('Locate in time', actions.locate, 'button subtle');
  locate.prepend(icon('fit'));
  root.append(
    stats,
    el(
      'p',
      { class: 'wb-inspector-denominator' },
      'Function share of window weight, across every call path.',
    ),
    el('div', { class: 'wb-inspector-actions' }, focus, locate),
  );
  const tabs = el('div', {
    class: 'wb-inspector-tabs',
    role: 'tablist',
    'aria-label': 'Function details',
  });
  const panel = el('div', {
    class: 'wb-inspector-panel',
    role: 'tabpanel',
    id: 'wb-detail-panel',
  });
  function tab(name: typeof state.inspectorTab) {
    if (signal.aborted) return;
    state.inspectorTab = name;
    context.persist();
    draw();
  }
  for (const [name, label] of [
    ['source', 'Source'],
    ['callers', 'Callers'],
    ['evidence', 'Evidence'],
    ['mapping', 'Mapped origins'],
  ] as const) {
    const control = button(label, () => tab(name), 'wb-inspector-tab');
    control.setAttribute('role', 'tab');
    control.id = `wb-detail-${name}`;
    control.setAttribute('aria-controls', panel.id);
    control.setAttribute('data-wb-inspector-tab', name);
    tabs.append(control);
  }
  const alternatives = () =>
    el(
      'div',
      { class: 'wb-alternatives' },
      button('Inspect callers', () => tab('callers'), 'button subtle'),
      button('Inspect evidence', () => tab('evidence'), 'button subtle'),
    );
  let page = 0;
  let pending: AbortController | undefined;
  function draw() {
    if (signal.aborted) return;
    const epoch = ++page;
    pending?.abort();
    for (const control of tabs.querySelectorAll<HTMLButtonElement>('button')) {
      const active = control.dataset.wbInspectorTab === state.inspectorTab;
      control.setAttribute('aria-selected', String(active));
      control.tabIndex = active ? 0 : -1;
    }
    panel.setAttribute('aria-labelledby', `wb-detail-${state.inspectorTab}`);
    if (state.inspectorTab === 'callers') {
      panel.replaceChildren(callers(selected!, actions.select));
      return;
    }
    if (
      state.inspectorTab === 'source' &&
      !hasInspectableSource(selected!.file, selected!.code_id)
    ) {
      panel.replaceChildren(
        el(
          'div',
          { class: 'wb-availability' },
          el('h3', {}, 'No mapped source'),
          el('p', {}, 'This frame has no recorded source location.'),
          alternatives(),
        ),
      );
      return;
    }
    panel.replaceChildren(
      el(
        'p',
        { class: 'wb-read-status', role: 'status' },
        state.inspectorTab === 'source'
          ? 'Loading source…'
          : state.inspectorTab === 'mapping'
            ? 'Loading mapped origins…'
            : 'Loading evidence…',
      ),
    );
    pending = new AbortController();
    const lifetime = linkedLifetime(signal, pending.signal);
    const read =
      state.inspectorTab === 'source'
        ? sourceContent(context, selected!, lifetime)
        : state.inspectorTab === 'mapping'
          ? mappedOrigins(
              context,
              selected!,
              data.window,
              data.clock_domain,
              lifetime,
            )
          : evidenceContent(context, selected!, lifetime);
    void read
      .then((content) => {
        if (!lifetime.aborted && epoch === page) panel.replaceChildren(content);
      })
      .catch((error: unknown) => {
        if (lifetime.aborted || epoch !== page) return;
        const unavailable =
          state.inspectorTab === 'source'
            ? sourceUnavailable(error)
            : undefined;
        if (unavailable) {
          panel.replaceChildren(
            el(
              'div',
              { class: 'wb-availability' },
              el('h3', {}, unavailable.title),
              el('p', {}, unavailable.message),
              alternatives(),
            ),
          );
          return;
        }
        panel.replaceChildren(
          el(
            'div',
            { class: 'wb-availability', role: 'alert' },
            el(
              'h3',
              {},
              state.inspectorTab === 'source'
                ? 'Source could not be loaded'
                : state.inspectorTab === 'mapping'
                  ? 'Mapped origins could not be loaded'
                  : 'Evidence could not be loaded',
            ),
            el('p', {}, error instanceof Error ? error.message : String(error)),
            button('Try again', draw, 'button'),
            alternatives(),
          ),
        );
      });
  }
  tabs.addEventListener('keydown', (event) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const values = ['source', 'callers', 'evidence', 'mapping'] as const;
    const index = values.indexOf(state.inspectorTab);
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? values.length - 1
          : (index + (event.key === 'ArrowLeft' ? values.length - 1 : 1)) %
            values.length;
    tab(values[next]);
    tabs
      .querySelector<HTMLButtonElement>(
        `[data-wb-inspector-tab="${values[next]}"]`,
      )
      ?.focus();
  });
  root.append(tabs, panel);
  const count = button(
    'Count function calls',
    () =>
      context.countFunctions([
        {
          name: selected.function,
          file: selected.file ?? undefined,
          line: selected.line ?? undefined,
        },
      ]),
    'button subtle',
  );
  root.append(el('div', { class: 'wb-inspector-footer' }, count));
  draw();
  return root;
}

function callers(
  selected: SelectedFunction,
  select: (id: string) => void,
): HTMLElement {
  const content = el('div', { class: 'wb-callers' });
  if (!selected.callers.length)
    content.append(
      el(
        'p',
        { class: 'wb-read-status' },
        selected.sample_count
          ? 'This function is at the root of its captured stacks.'
          : 'No samples for this function in this time window.',
      ),
    );
  for (const caller of selected.callers) {
    const control = button('', () => select(caller.id), 'wb-caller');
    control.append(
      el('span', { class: 'mono' }, caller.function),
      el(
        'span',
        { class: 'wb-caller-file' },
        caller.file ? shortPath(caller.file) : caller.module || 'Runtime',
      ),
      el(
        'span',
        { class: 'mono' },
        weight(caller.weight, selected.weight_unit),
      ),
    );
    content.append(control);
  }
  if (selected.callers_truncated)
    content.append(
      el(
        'p',
        { class: 'wb-read-status' },
        `${selected.callers.length} caller groups shown.`,
      ),
    );
  return content;
}

async function sourceContent(
  context: ViewContext,
  selected: SelectedFunction,
  signal: AbortSignal,
): Promise<HTMLElement> {
  const data = await operation(
    'source.read',
    {
      run_id: context.run.id,
      path: selected.file!,
      line: Math.max(1, (selected.line ?? 1) - 6),
      code_id: selected.code_id ?? undefined,
      count: 80,
    },
    signal,
  );
  const state = sourcePresentation(data);
  const content = el('div', { class: 'wb-source' });
  content.append(
    el(
      'div',
      { class: 'wb-source-header' },
      el('span', {}, shortPath(data.path)),
      badge(state.origin),
    ),
    el(
      'p',
      { class: 'wb-source-confidence' },
      state.identity,
      ' · ',
      state.mapping,
    ),
  );
  if (state.content === 'lines') {
    const code = el('div', {
      class: 'wb-code',
      tabindex: 0,
      'aria-label': `Source ${data.path}`,
    });
    for (const [index, line] of data.content.split('\n').entries()) {
      const number = data.start_line + index;
      code.append(
        el(
          'div',
          {
            class: `wb-code-line${number === selected.line ? ' recorded-position' : ''}`,
          },
          el('span', { class: 'wb-line-number', 'aria-hidden': true }, number),
          el('code', {}, line || ' '),
        ),
      );
    }
    content.append(code);
  } else
    content.append(
      el(
        'p',
        { class: 'wb-read-status' },
        state.content === 'empty-file'
          ? 'This file is empty.'
          : state.content === 'outside-window'
            ? 'The recorded position is outside this source window.'
            : 'Source contents are unavailable.',
      ),
    );
  if (selected.line)
    content.append(
      el(
        'p',
        { class: 'wb-source-position' },
        `Recorded position · line ${selected.line}. Line-level cost is not measured.`,
      ),
    );
  content.append(
    el(
      'details',
      { class: 'wb-source-details' },
      el('summary', {}, 'Source identity'),
      el('p', {}, state.note),
      data.diagnostic ? el('p', {}, data.diagnostic) : null,
      el('p', { class: 'mono' }, data.path),
    ),
  );
  const open = button(
    'Open full source',
    () => context.source(selected.file!, selected.line, selected.code_id),
    'button subtle',
  );
  open.append(icon('expand'));
  content.append(open);
  return content;
}

async function evidenceContent(
  context: ViewContext,
  selected: SelectedFunction,
  signal: AbortSignal,
): Promise<HTMLElement> {
  const content = el('div', { class: 'wb-evidence' });
  content.append(
    el(
      'p',
      { class: 'wb-read-status' },
      `${numeric(selected.evidence_count)} captured samples in this window`,
    ),
  );
  if (!selected.evidence_ids.length) return content;
  const data = await operation(
    'evidence.get',
    { run_id: context.run.id, ids: selected.evidence_ids },
    signal,
  );
  for (const record of data.records) {
    const label =
      record.kind === 'sample' &&
      typeof record.weight === 'number' &&
      record.weight_unit
        ? weight(record.weight, record.weight_unit)
        : record.kind;
    const details = el(
      'details',
      { class: 'wb-evidence-record' },
      el('summary', {}, `${label} · PID ${record.pid}`),
    );
    details.addEventListener('toggle', () => {
      if (details.open && !details.querySelector('pre'))
        details.append(
          el('p', { class: 'mono' }, record.id),
          el('pre', {}, JSON.stringify(record, null, 2)),
        );
    });
    content.append(details);
  }
  if (selected.evidence_truncated)
    content.append(
      el(
        'p',
        { class: 'wb-read-status' },
        `${selected.evidence_ids.length} sample records shown.`,
      ),
    );
  return content;
}
