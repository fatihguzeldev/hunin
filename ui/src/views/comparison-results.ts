import type { Comparison } from '../api';
import { badge, button, el, empty, field } from '../dom';
import { numeric, percentage } from '../format';
import { sourceIdentityMetadata } from '../metadata';
import type { ViewContext } from '../investigation/view';

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
    el(
      'p',
      { class: 'metric-note' },
      passed == null
        ? 'Correctness validation is unavailable.'
        : validation === 'exit_code_and_stdout'
          ? 'Declared exit code and stdout checks'
          : 'Exit code check only; output correctness is unvalidated.',
    ),
    el('pre', {}, JSON.stringify(value, null, 2)),
  );
}

function identities(run: Comparison['before']): HTMLElement {
  const identity = sourceIdentityMetadata(run.source);
  return el(
    'div',
    { class: 'stack' },
    el('p', { class: 'metric-note' }, 'Source snapshot fingerprint'),
    el(
      'p',
      { class: 'metric-note mono' },
      identity.sourceFingerprint || 'Unavailable',
    ),
    el('p', { class: 'metric-note' }, 'Launched executable SHA-256'),
    el(
      'p',
      { class: 'metric-note mono' },
      identity.executableSha256 || 'Unavailable',
    ),
    el(
      'p',
      { class: 'metric-note' },
      `${run.runtime} · runtime version ${identity.runtimeVersion || 'unavailable'}`,
    ),
    el(
      'p',
      { class: 'metric-note' },
      identity.buildSourceVerified === true
        ? 'Recorded source/executable relationship verified.'
        : identity.buildSourceVerified === false
          ? 'Source/executable relationship unverified.'
          : 'Source/executable relationship unknown.',
    ),
  );
}

function metrics(data: Comparison, context: ViewContext): HTMLElement {
  const selection = el('select');
  const key = (row: Comparison['metrics'][number]) =>
    JSON.stringify([row.name, row.unit, row.scope]);
  selection.append(
    el('option', { value: '' }, 'All recorded metrics'),
    ...data.metrics.map((row) =>
      el(
        'option',
        { value: key(row) },
        `${row.name} · ${row.unit} · ${row.scope}`,
      ),
    ),
  );
  const saved = context.investigation.comparison.metric;
  if (saved && !data.metrics.some((row) => key(row) === saved))
    selection.append(
      el(
        'option',
        { value: saved },
        'Selected metric · unavailable in this pair',
      ),
    );
  selection.value = saved ?? '';
  const content = el('div');
  function render(): void {
    const rows = data.metrics.filter(
      (row) => !selection.value || key(row) === selection.value,
    );
    content.replaceChildren(
      rows.length
        ? el(
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
                  el('th', {}, 'Metric / unit / scope'),
                  el('th', { class: 'numeric' }, 'Before'),
                  el('th', { class: 'numeric' }, 'After'),
                  el('th', { class: 'numeric' }, 'Delta'),
                  el('th', { class: 'numeric' }, 'Change'),
                ),
              ),
              el(
                'tbody',
                {},
                ...rows.map((row) =>
                  el(
                    'tr',
                    {},
                    el(
                      'td',
                      {},
                      row.name,
                      el('small', { class: 'source-label' }, row.unit),
                      el(
                        'small',
                        { class: 'source-label' },
                        `Scope: ${row.scope}`,
                      ),
                      el('small', { class: 'source-label' }, row.basis),
                      row.measurement_quality
                        ? badge(
                            row.measurement_quality.replaceAll('_', ' '),
                            row.comparable === false ? 'warn' : '',
                          )
                        : null,
                      row.diagnostic
                        ? el('small', { class: 'source-label' }, row.diagnostic)
                        : null,
                      row.before_observations !== undefined
                        ? el(
                            'small',
                            { class: 'source-label' },
                            `${row.before_observations} before / ${row.after_observations ?? 'unknown'} after observations`,
                          )
                        : null,
                    ),
                    el('td', { class: 'numeric' }, numeric(row.before)),
                    el('td', { class: 'numeric' }, numeric(row.after)),
                    el(
                      'td',
                      { class: 'numeric' },
                      row.delta == null
                        ? '—'
                        : `${row.delta > 0 ? '+' : ''}${numeric(row.delta)}`,
                    ),
                    el(
                      'td',
                      { class: 'numeric' },
                      percentage(row.percent_change),
                    ),
                  ),
                ),
              ),
            ),
          )
        : empty(
            'Selected metric is unavailable',
            'Choose another recorded metric or inspect the recording diagnostics. Missing measurements are not zero.',
          ),
    );
  }
  selection.addEventListener('change', () => {
    context.investigation.comparison.metric = selection.value || undefined;
    context.persist();
    render();
  });
  render();
  return el(
    'section',
    { class: 'panel' },
    el(
      'div',
      { class: 'panel-head' },
      el('h2', {}, 'Measured changes'),
      badge(
        data.comparable ? 'workload checks passed' : 'limited',
        data.comparable ? 'good' : 'warn',
      ),
    ),
    el('div', { class: 'panel-body' }, field('Metric to inspect', selection)),
    content,
  );
}

function hotspots(data: Comparison): HTMLElement {
  const selection = data.hotspot_selection;
  const content = el('div');
  const status = el('p', { class: 'metric-note', role: 'status' });
  const size = 50;
  let offset = 0;
  const first = button(
    'First hotspots',
    () => {
      offset = 0;
      render();
    },
    'button subtle small',
  );
  const previous = button(
    'Previous hotspots',
    () => {
      offset = Math.max(0, offset - size);
      render();
    },
    'button subtle small',
  );
  const next = button(
    'More hotspots',
    () => {
      offset += size;
      render();
    },
    'button subtle small',
  );
  function render(): void {
    const rows = data.hotspots.slice(offset, offset + size);
    first.disabled = previous.disabled = offset === 0;
    next.disabled = offset + size >= data.hotspots.length;
    status.textContent = rows.length
      ? `Hotspots ${offset + 1}–${offset + rows.length} of ${data.hotspots.length} returned rows`
      : 'No returned hotspots for this sample-unit selection.';
    content.replaceChildren(
      rows.length
        ? el(
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
                  el('th', {}, 'Observed symbol / presence'),
                  el('th', { class: 'numeric' }, 'Before'),
                  el('th', { class: 'numeric' }, 'After'),
                  el('th', { class: 'numeric' }, 'Delta'),
                ),
              ),
              el(
                'tbody',
                {},
                ...rows.map((row) =>
                  el(
                    'tr',
                    {},
                    el(
                      'td',
                      {},
                      el('span', { class: 'mono' }, row.function),
                      el(
                        'small',
                        { class: 'source-label' },
                        row.file || row.module || 'Source/module unavailable',
                      ),
                      badge(
                        row.presence === 'both_observed'
                          ? 'observed in both'
                          : row.presence === 'before_only_observed'
                            ? 'not observed after'
                            : row.presence === 'after_only_observed'
                              ? 'newly observed after'
                              : 'presence unknown',
                        row.presence === 'unknown' || row.presence === undefined
                          ? 'warn'
                          : '',
                      ),
                    ),
                    el('td', { class: 'numeric' }, numeric(row.before)),
                    el('td', { class: 'numeric' }, numeric(row.after)),
                    el(
                      'td',
                      { class: 'numeric' },
                      row.delta == null
                        ? '—'
                        : `${row.delta > 0 ? '+' : ''}${numeric(row.delta)}`,
                    ),
                  ),
                ),
              ),
            ),
          )
        : empty(
            'No hotspot rows',
            'Inspect the selected sample unit and collection diagnostics. Absence does not establish a zero execution cost.',
          ),
    );
  }
  render();
  return el(
    'section',
    { class: 'panel' },
    el(
      'div',
      { class: 'panel-head' },
      el('h2', {}, 'Observed hotspot changes'),
      badge(
        selection?.weight_unit || data.hotspots[0]?.unit || 'unit unavailable',
      ),
    ),
    el(
      'div',
      { class: 'panel-body stack' },
      el(
        'p',
        { class: 'metric-note' },
        'Weights are profiler samples grouped by function/file/module across measured code versions. Names do not establish semantic equivalence; a function not observed in samples is not proven absent from the program.',
      ),
      el(
        'p',
        { class: 'metric-note' },
        selection
          ? `${selection.before_total_functions} before / ${selection.after_total_functions} after symbol groups. At most ${selection.limit_per_recording} groups are returned per recording. Selected unit: before ${selection.before_available ? 'available' : 'unavailable'}, after ${selection.after_available ? 'available' : 'unavailable'}.`
          : 'This service did not provide hotspot coverage metadata.',
      ),
      selection?.before_truncated || selection?.after_truncated
        ? el(
            'p',
            { class: 'notice' },
            'Hotspot rank windows are truncated. Missing counterparts outside a returned window remain unknown; they are not measured zero.',
          )
        : null,
      el(
        'p',
        { class: 'metric-note' },
        '— indicates an unavailable weight or delta. A reported zero for an unobserved counterpart requires a complete returned symbol inventory and available sample unit.',
      ),
    ),
    content,
    el('div', { class: 'panel-body toolbar' }, first, previous, next, status),
  );
}

export function comparisonResults(
  data: Comparison,
  context: ViewContext,
): HTMLElement {
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
    ...data.limitations.map((reason) =>
      el('p', { class: 'metric-note' }, reason),
    ),
    el(
      'div',
      { class: 'evidence-grid' },
      ...(['before', 'after'] as const).map((side) =>
        el(
          'section',
          { class: 'panel' },
          el(
            'div',
            { class: 'panel-head' },
            el('h2', {}, side === 'before' ? 'Before' : 'After'),
            el('small', {}, data[side].name),
          ),
          el(
            'div',
            { class: 'panel-body' },
            correctness(data.correctness[side]),
          ),
        ),
      ),
    ),
    el(
      'section',
      { class: 'panel' },
      el(
        'div',
        { class: 'panel-head' },
        el('h2', {}, 'Source and executable identities'),
      ),
      el(
        'p',
        { class: 'panel-body metric-note' },
        'Source/build changes can be the optimization under comparison. These retained identities are separate from matching workload inputs and correctness checks.',
      ),
      el(
        'div',
        { class: 'evidence-grid panel-body' },
        ...(['before', 'after'] as const).map((side) =>
          el(
            'div',
            { class: 'stack' },
            el(
              'h3',
              {},
              side === 'before' ? 'Before identities' : 'After identities',
            ),
            identities(data[side]),
          ),
        ),
      ),
    ),
    metrics(data, context),
    hotspots(data),
  );
}
