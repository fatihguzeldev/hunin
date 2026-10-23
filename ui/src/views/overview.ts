import { operation, type Run, type Summary } from '../api';
import { badge, el, empty } from '../dom';
import { duration, numeric } from '../format';
import { hasLocalSource } from '../source-path';

export function runTone(run: Run): string {
  return { completed: 'good', failed: 'bad', canceled: 'warn', running: 'warn' }[run.status];
}

export function weight(value: number, unit: string): string {
  return unit === 'microseconds' ? duration(value * 1_000) : `${numeric(value)} ${unit}`;
}

function stat(label: string, value: string, note: string): HTMLElement {
  return el('div', { class: 'stat' }, el('div', { class: 'stat-label' }, label),
    el('div', { class: 'stat-value' }, value), el('div', { class: 'stat-note' }, note));
}

export async function overview(run: Run, summary: Summary, source: (file: string, line?: number) => void): Promise<HTMLElement> {
  const data = await operation('profiles.hotspots', { run_id: run.id, limit: 25 });
  const body = el('div', { class: 'stack' });
  const elapsed = run.finished_at_us ? duration((run.finished_at_us - run.started_at_us) * 1_000) : 'Recording';
  body.append(el('div', { class: 'stats' },
    stat('Elapsed', elapsed, 'wall clock · whole run'),
    stat('Sampled profile', weight(data.total_weight, data.weight_unit), `${numeric(summary.sample_count)} stack samples`),
    stat('Explicit spans', numeric(summary.span_count), 'SDK observations'),
    stat('Relationships', numeric(summary.relation_count), 'recorded evidence')));
  if (run.correctness?.passed === false) body.append(el('p', { class: 'notice' }, run.correctness.actual_exit_code == null
    ? `Workload check incomplete: no exit result is available for this ${run.status} recording. ${run.correctness.reason ?? ''}`
    : `Workload check failed: expected exit ${run.correctness.expected_exit_code ?? 'unknown'}, observed ${run.correctness.actual_exit_code}. ${run.correctness.reason ?? ''}`));
  const rows = el('tbody');
  for (const item of data.hotspots) {
    const name = hasLocalSource(item.file) ? el('button', { class: 'text-button mono', type: 'button' }, item.function) : el('span', { class: 'mono' }, item.function);
    if (hasLocalSource(item.file)) name.addEventListener('click', () => source(item.file!, item.line));
    const bar = el('span');
    bar.style.width = `${data.total_weight > 0 ? Math.min(100, item.self_weight / data.total_weight * 100) : 0}%`;
    rows.append(el('tr', {}, el('td', {}, name,
      el('span', { class: 'source-label mono' }, item.file ? `${item.file}${item.line ? `:${item.line}` : ''}` : 'Source unavailable')),
    el('td', { class: 'numeric' }, weight(item.self_weight, item.weight_unit), el('div', { class: 'weight-bar' }, bar)),
    el('td', { class: 'numeric' }, weight(item.inclusive_weight, item.weight_unit))));
  }
  const panel = el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Hot functions'), el('small', {}, 'Top 25 · self weight')));
  if (data.hotspots.length) panel.append(el('div', { class: 'table-scroll' }, el('table', {},
    el('thead', {}, el('tr', {}, el('th', {}, 'Function / source'), el('th', { class: 'numeric' }, 'Self'), el('th', { class: 'numeric' }, 'Inclusive'))), rows)));
  else panel.append(empty('No accepted stack samples', 'Inspect the recording diagnostics and collector capabilities.'));
  body.append(panel, el('p', { class: 'metric-note' }, 'Stack weights describe sampled execution. Inclusive weights include descendants and must not be added together.'));
  const diagnostics = [...run.diagnostics, ...summary.diagnostics];
  if (diagnostics.length) body.append(el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Collection notes'), badge(String(diagnostics.length))),
    el('div', { class: 'panel-body stack' }, ...diagnostics.map(message => el('p', { class: 'metric-note' }, message)))));
  return body;
}
