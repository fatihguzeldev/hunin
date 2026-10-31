import { weight, weightShare } from '../profile-weight';
import { captureQuality, targetCheck } from '../recordings/presentation';
import { operation, type Operations, type Run, type Summary } from '../api';
import { badge, el, empty } from '../dom';
import { queryFailureMessage } from '../recordings/details';
import { duration, numeric } from '../format';
import { hasInspectableSource } from '../source/paths';

function stat(label: string, value: string, note: string): HTMLElement {
  return el(
    'div',
    { class: 'stat' },
    el('div', { class: 'stat-label' }, label),
    el('div', { class: 'stat-value' }, value),
    el('div', { class: 'stat-note' }, note),
  );
}

export async function overview(
  run: Run,
  summary: Summary | undefined,
  source: (file: string, line?: number | null, codeId?: string | null) => void,
  signal?: AbortSignal,
): Promise<HTMLElement> {
  const capture = captureQuality(run);
  const target = targetCheck(run);
  let data: Operations['profiles.hotspots']['output'] | undefined;
  let hotspotError: unknown;
  try {
    data = await operation(
      'profiles.hotspots',
      {
        run_id: run.id,
        limit: 25,
      },
      signal,
    );
  } catch (error) {
    hotspotError = error;
  }
  signal?.throwIfAborted();
  const body = el('div', { class: 'stack' });
  const imported = run.runtime === 'imported';
  const elapsed = imported
    ? '—'
    : run.finished_at_us !== null
      ? duration((run.finished_at_us - run.started_at_us) * 1_000)
      : run.status === 'running'
        ? 'Recording'
        : '—';
  body.append(
    el(
      'div',
      { class: 'stats' },
      stat(
        'Recording duration',
        elapsed,
        imported
          ? 'Target duration not recorded'
          : 'Includes setup and capture',
      ),
      stat(
        'Profile weight',
        data ? weight(data.total_weight, data.weight_unit) : 'Unavailable',
        summary
          ? `${numeric(summary.sample_count)} stack samples`
          : 'Stack sample count unavailable',
      ),
      stat(
        'Recorded spans',
        summary ? numeric(summary.span_count) : 'Unavailable',
        'Timeline intervals',
      ),
      stat(
        'Recorded links',
        summary ? numeric(summary.relation_count) : 'Unavailable',
        'Links between observations',
      ),
    ),
  );
  body.append(
    el(
      'div',
      { class: 'evidence-grid' },
      ...[
        { label: 'Capture', state: capture },
        { label: 'Target', state: target },
      ].map(({ label, state }) =>
        el(
          'section',
          { class: 'panel' },
          el(
            'div',
            { class: 'panel-head' },
            el('h2', {}, label),
            badge(state.label, state.tone),
          ),
          el('p', { class: 'panel-body' }, state.detail),
        ),
      ),
    ),
  );
  const rows = el('tbody');
  for (const item of data?.hotspots ?? []) {
    const name = hasInspectableSource(item.file, item.code_id)
      ? el(
          'button',
          { class: 'text-button mono', type: 'button' },
          item.function,
        )
      : el('span', { class: 'mono' }, item.function);
    if (hasInspectableSource(item.file, item.code_id))
      name.addEventListener('click', () =>
        source(item.file!, item.line, item.code_id ?? undefined),
      );
    const bar = el('span');
    bar.style.width = `${weightShare(item.self_weight, data?.total_weight ?? null) ?? 0}%`;
    rows.append(
      el(
        'tr',
        {},
        el(
          'td',
          {},
          name,
          el(
            'span',
            { class: 'source-label mono' },
            item.file
              ? `${item.file}${item.line ? `:${item.line}` : ''}`
              : 'Source unavailable',
          ),
        ),
        el(
          'td',
          { class: 'numeric' },
          weight(item.self_weight, item.weight_unit),
          el('div', { class: 'weight-bar' }, bar),
        ),
        el(
          'td',
          { class: 'numeric' },
          weight(item.inclusive_weight, item.weight_unit),
        ),
      ),
    );
  }
  const panel = el(
    'section',
    { class: 'panel' },
    el(
      'div',
      { class: 'panel-head' },
      el('h2', {}, 'Hot functions'),
      el('small', {}, 'Top 25 · self weight'),
    ),
  );
  if (!data)
    panel.append(
      el(
        'p',
        { class: 'panel-body notice', role: 'alert' },
        `Hot functions unavailable: ${queryFailureMessage(hotspotError)}`,
      ),
    );
  else if (data.hotspots.length)
    panel.append(
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
              el('th', {}, 'Function / source'),
              el('th', { class: 'numeric' }, 'Self'),
              el('th', { class: 'numeric' }, 'Inclusive'),
            ),
          ),
          rows,
        ),
      ),
    );
  else
    panel.append(
      empty(
        run.status === 'running'
          ? 'Waiting for stack samples'
          : 'No stack samples',
        run.runtime === 'none'
          ? 'Automatic profiling was disabled for this recording.'
          : 'No stack samples have been accepted for this recording.',
      ),
    );
  body.append(
    panel,
    el(
      'p',
      { class: 'metric-note' },
      'Self excludes called functions. Inclusive includes them; overlapping inclusive weights cannot be added together.',
    ),
  );
  const diagnostics = [...run.diagnostics, ...(summary?.diagnostics ?? [])];
  if (diagnostics.length)
    body.append(
      el(
        'details',
        { class: 'panel' },
        el(
          'summary',
          { class: 'panel-head' },
          el('span', {}, 'Collection notes'),
          badge(String(diagnostics.length)),
        ),
        el(
          'div',
          { class: 'panel-body stack' },
          ...diagnostics.map((message) =>
            el('p', { class: 'metric-note' }, message),
          ),
        ),
      ),
    );
  body.append(
    el(
      'details',
      { class: 'panel' },
      el('summary', { class: 'panel-body' }, 'Workload check details'),
      el(
        'pre',
        { class: 'panel-body' },
        JSON.stringify(run.correctness, null, 2),
      ),
    ),
    el(
      'details',
      { class: 'panel' },
      el('summary', { class: 'panel-body' }, 'Source and runtime details'),
      el('pre', { class: 'panel-body' }, JSON.stringify(run.source, null, 2)),
    ),
  );
  return body;
}
