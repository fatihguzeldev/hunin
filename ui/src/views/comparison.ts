import { operation, type Comparison } from '../api';
import { linkedRequest } from '../requests';
import { recordingHistory } from '../recording-history';
import { badge, button, el, empty, errorView, field, loading } from '../dom';
import { numeric, percentage } from '../format';
import type { ViewContext } from '../workspace';

function correctness(value: unknown): HTMLElement {
  const passed =
    value && typeof value === 'object' && 'passed' in value
      ? value.passed
      : null;
  const validation =
    value && typeof value === 'object' && 'validation' in value
      ? value.validation
      : null;
  return el(
    'div',
    { class: 'stack' },
    badge(
      passed === true
        ? 'correctness passed'
        : passed === false
          ? 'correctness failed'
          : 'correctness unknown',
      passed === true ? 'good' : passed === false ? 'bad' : 'warn',
    ),
    passed == null
      ? el(
          'p',
          { class: 'metric-note' },
          'Correctness validation is unavailable.',
        )
      : validation === 'exit_code_and_stdout'
        ? el(
            'p',
            { class: 'metric-note' },
            'Declared exit code and stdout checks',
          )
        : el(
            'p',
            { class: 'metric-note' },
            'Exit code check only; output correctness is unvalidated.',
          ),
    el('pre', {}, JSON.stringify(value, null, 2)),
  );
}

function results(data: Comparison): HTMLElement {
  return el(
    'div',
    { class: 'stack' },
    el(
      'p',
      { class: 'notice' },
      data.comparable
        ? 'The recorded workload and settings meet the backend comparison checks. Measurement noise and collection overhead still limit the claim.'
        : 'These runs do not meet the comparison checks. Changes below describe the observations and cannot be presented as a validated optimization.',
    ),
    ...data.reasons.map((reason) => el('p', { class: 'metric-note' }, reason)),
    ...(data.limitations ?? []).map((reason) =>
      el('p', { class: 'metric-note' }, reason),
    ),
    el(
      'div',
      { class: 'evidence-grid' },
      el(
        'section',
        { class: 'panel' },
        el(
          'div',
          { class: 'panel-head' },
          el('h2', {}, 'Before'),
          el('small', {}, data.before.name),
        ),
        el(
          'div',
          { class: 'panel-body' },
          correctness(data.correctness.before),
        ),
      ),
      el(
        'section',
        { class: 'panel' },
        el(
          'div',
          { class: 'panel-head' },
          el('h2', {}, 'After'),
          el('small', {}, data.after.name),
        ),
        el('div', { class: 'panel-body' }, correctness(data.correctness.after)),
      ),
    ),
    el(
      'section',
      { class: 'panel' },
      el(
        'div',
        { class: 'panel-head' },
        el('h2', {}, 'Measured changes'),
        badge(
          data.comparable ? 'comparable' : 'limited',
          data.comparable ? 'good' : 'warn',
        ),
      ),
      el(
        'div',
        { class: 'table-scroll' },
        el(
          'table',
          {},
          el(
            'thead',
            {},
            el(
              'tr',
              {},
              el('th', {}, 'Metric / unit'),
              el('th', { class: 'numeric' }, 'Before'),
              el('th', { class: 'numeric' }, 'After'),
              el('th', { class: 'numeric' }, 'Delta'),
              el('th', { class: 'numeric' }, 'Change'),
            ),
          ),
          el(
            'tbody',
            {},
            ...data.metrics.map((metric) =>
              el(
                'tr',
                {},
                el(
                  'td',
                  {},
                  metric.name,
                  el('small', { class: 'source-label' }, metric.unit),
                  metric.measurement_quality
                    ? badge(
                        metric.measurement_quality,
                        metric.comparable === false ? 'warn' : '',
                      )
                    : null,
                  metric.diagnostic
                    ? el('small', { class: 'source-label' }, metric.diagnostic)
                    : null,
                  metric.before_observations !== undefined
                    ? el(
                        'small',
                        { class: 'source-label' },
                        `${metric.before_observations} before / ${metric.after_observations ?? 'unknown'} after observations`,
                      )
                    : null,
                ),
                el('td', { class: 'numeric' }, numeric(metric.before)),
                el('td', { class: 'numeric' }, numeric(metric.after)),
                el(
                  'td',
                  { class: 'numeric' },
                  metric.delta == null
                    ? '—'
                    : `${metric.delta > 0 ? '+' : ''}${numeric(metric.delta)}`,
                ),
                el(
                  'td',
                  { class: 'numeric' },
                  percentage(metric.percent_change),
                ),
              ),
            ),
          ),
        ),
      ),
    ),
    el(
      'details',
      { class: 'panel' },
      el(
        'summary',
        { class: 'panel-body' },
        'Hotspot differences · backend result',
      ),
      el(
        'pre',
        { class: 'panel-body' },
        JSON.stringify(data.hotspots, null, 2),
      ),
    ),
  );
}

export async function comparison(context: ViewContext): Promise<HTMLElement> {
  if (context.runs.length < 2)
    return empty(
      'Record the same workload again',
      'Comparison needs at least two recordings. Run your workload after a change, then select a baseline here.',
    );
  const state = context.investigation.comparison;
  state.after ??= context.run.id;
  state.before ??= context.runs.find((run) => run.id !== state.after)?.id;
  const known = new Map(context.runs.map((run) => [run.id, run]));
  const before = el('select');
  const after = el('select');
  const content = el('div');
  let request: AbortController | undefined;
  let epoch = 0;
  const compare = button(
    'Compare recordings',
    () => {
      void load();
    },
    'button primary',
  );
  function populate(): void {
    const allowed = new Set([
      ...context.runs.map((run) => run.id),
      state.before,
      state.after,
    ]);
    for (const id of known.keys()) if (!allowed.has(id)) known.delete(id);
    for (const select of [before, after]) {
      select.replaceChildren(
        ...[...known.values()].map((run) =>
          el(
            'option',
            { value: run.id },
            `${run.name} · ${run.id.slice(0, 8)}`,
          ),
        ),
      );
      const selected = select === before ? state.before : state.after;
      if (selected && !known.has(selected))
        select.append(
          el(
            'option',
            { value: selected },
            `${selected} · metadata not loaded`,
          ),
        );
      select.value = selected ?? '';
    }
  }
  function changed(): void {
    state.before = before.value;
    state.after = after.value;
    epoch++;
    request?.abort();
    compare.disabled = false;
    context.persist();
    content.replaceChildren(
      empty(
        'Comparison selection changed',
        'Compare these recordings to read their current selected results and limitations.',
      ),
    );
  }
  populate();
  before.addEventListener('change', changed);
  after.addEventListener('change', changed);
  const historyPicker = recordingHistory(context.signal, (run, side) => {
    known.set(run.id, run);
    state[side] = run.id;
    populate();
    changed();
  });
  const body = el(
    'div',
    { class: 'stack' },
    el(
      'div',
      { class: 'toolbar' },
      field('Before · baseline', before),
      field('After · candidate', after),
      compare,
    ),
    historyPicker,
    content,
  );
  async function load(): Promise<void> {
    if (context.signal.aborted) return;
    const pair = { before: before.value, after: after.value };
    if (!pair.before || !pair.after || pair.before === pair.after) {
      content.replaceChildren(
        empty(
          'Choose two different recordings',
          'A recording cannot serve as its own before/after comparison.',
        ),
      );
      return;
    }
    const version = ++epoch;
    request?.abort();
    request = new AbortController();
    const linked = linkedRequest(context.signal, request.signal);
    state.before = pair.before;
    state.after = pair.after;
    context.persist();
    compare.disabled = true;
    content.replaceChildren(loading('Assessing comparability…'));
    try {
      await Promise.all(
        [...new Set([pair.before, pair.after])]
          .filter((id) => !known.has(id))
          .map(async (id) => {
            const { run } = await operation(
              'runs.get',
              { run_id: id },
              linked.signal,
            );
            if (!linked.signal.aborted) known.set(run.id, run);
          }),
      );
      if (version !== epoch || linked.signal.aborted) return;
      populate();
      const data = await operation('comparisons.create', pair, linked.signal);
      if (
        version !== epoch ||
        linked.signal.aborted ||
        before.value !== pair.before ||
        after.value !== pair.after
      )
        return;
      content.replaceChildren(results(data));
    } catch (error) {
      if (version === epoch && !linked.signal.aborted)
        content.replaceChildren(
          errorView(error, () => {
            void load();
          }),
        );
    } finally {
      linked.finish();
      if (version === epoch) compare.disabled = false;
    }
  }
  await load();
  return body;
}
