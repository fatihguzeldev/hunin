import { operation } from '../api';
import { badge, button, el, empty, errorView, field, loading } from '../dom';
import { duration, numeric } from '../format';
import type { Observation } from '../record.generated';
import type { ViewContext } from '../workspace';
import { hasInspectableSource } from '../source-path';
import { isModuleLifetime, observationLabel, referenceIds } from '../observation';
import { nativeEvidence } from '../native-evidence';

export async function timeline(context: ViewContext): Promise<HTMLElement> {
  const canvas = el('div');
  const detail = el('div');
  const rangeLabel = el('span', { class: 'metric-note' });
  const filter = el('select', { 'aria-label': 'Observation type' });
  for (const value of ['', 'sample', 'span', 'event', 'metric', 'context', 'relation']) filter.append(el('option', { value }, value || 'All observations'));
  const pid = el('input', { type: 'number', min: 0, placeholder: 'All processes', 'aria-label': 'Process ID' });
  const alignment = el('select', { 'aria-label': 'Clock alignment' });
  alignment.append(el('option', { value: 'aligned' }, 'Aligned timeline'), el('option', { value: 'unmapped' }, 'Unmapped clock evidence'));
  let bounds: { start_us: number; end_us: number } | undefined;
  let window: { start_us: number; end_us: number } | undefined;
  let cursor: string | undefined;
  let epoch = 0;
  const body = el('div', { class: 'stack' }, el('div', { class: 'toolbar' },
    button('Zoom in', () => zoom(.5), 'button small'), button('Zoom out', () => zoom(2), 'button small'),
    button('←', () => pan(-1), 'button small'), button('→', () => pan(1), 'button small'), button('Full run', () => { window = undefined; cursor = undefined; void load(); }, 'button small'),
    el('span', { class: 'spacer' }), rangeLabel),
    el('div', { class: 'toolbar' }, field('Observation', filter), field('Process ID', pid), field('Clock', alignment), button('Apply filter', () => { cursor = undefined; void load(); })),
    canvas, detail,
    el('p', { class: 'metric-note' }, 'Positions use recorded timestamps. Thread identifiers retain their namespace; equal values from different namespaces are separate lanes. Overlapping intervals alone do not establish a causal relationship. CPU samples are points, not measured function durations.'));
  function zoom(factor: number): void {
    if (!bounds || alignment.value === 'unmapped') return;
    const current = window ?? bounds;
    const center = (current.start_us + current.end_us) / 2;
    const half = Math.max(1, (current.end_us - current.start_us) * factor / 2);
    window = { start_us: Math.max(bounds.start_us, Math.round(center - half)), end_us: Math.min(bounds.end_us, Math.round(center + half)) };
    cursor = undefined;
    void load();
  }
  function pan(direction: number): void {
    if (!bounds || alignment.value === 'unmapped') return;
    const current = window ?? bounds;
    const width = current.end_us - current.start_us;
    const start = Math.max(bounds.start_us, Math.min(bounds.end_us - width, current.start_us + direction * width / 2));
    window = { start_us: Math.round(start), end_us: Math.round(start + width) };
    cursor = undefined;
    void load();
  }
  function inspect(record: Observation): void {
    const source = record.stack?.find(frame => hasInspectableSource(frame.file, frame.code_id));
    const references = referenceIds(record);
    detail.replaceChildren(el('section', { class: 'panel' },
      el('div', { class: 'panel-head' }, el('h2', {}, observationLabel(record)), badge(record.basis), badge(record.kind)),
      el('div', { class: 'panel-body stack' }, source ? button(`Open ${source.file}:${source.line ?? ''}`, () => context.source(source.file!, source.line, source.code_id), 'text-button mono') : null,
        nativeEvidence(record),
        references.length ? el('div', { class: 'toolbar' }, ...references.map(id => button(`Open reference ${id}`, () => {
          void operation('evidence.get', { run_id: context.run.id, ids: [id] }).then(data => {
            if (data.records.length) inspect(data.records[0]);
            else detail.append(el('p', { class: 'notice' }, `Reference ${id} is unavailable in this recording.`));
          }).catch(error => detail.append(errorView(error, () => inspect(record))));
        }, 'button small mono'))) : null,
        el('pre', {}, JSON.stringify(record, null, 2)))));
    detail.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }
  async function load(): Promise<void> {
    const version = ++epoch;
    canvas.replaceChildren(loading('Loading timeline window…'));
    try {
      const unmappedView = alignment.value === 'unmapped';
      const data = await operation('timeline.query', { run_id: context.run.id, ...(!unmappedView ? window : {}), alignment: unmappedView ? 'unmapped' : 'aligned', limit: 300, cursor, kind: filter.value || undefined, pid: pid.value === '' ? undefined : Number(pid.value) });
      if (version !== epoch) return;
      bounds ??= data.bounds ?? undefined;
      const current = window ?? data.bounds ?? { start_us: 0, end_us: 0 };
      const origin = bounds?.start_us ?? current.start_us;
      rangeLabel.textContent = unmappedView ? 'Unmapped clock · no epoch placement' : `${duration((current.start_us - (bounds?.start_us ?? 0)) * 1_000)} → ${duration((current.end_us - (bounds?.start_us ?? 0)) * 1_000)}`;
      if (!data.records.length) { canvas.replaceChildren(
        data.unaligned_count ? el('p', { class: 'notice' }, `${data.unaligned_count} observations have unmapped clocks. They are excluded from the aligned timeline; retained artifacts preserve their original evidence.`) : el('span'),
        empty('No observations in this window', 'Change the time window or observation filter.')); return; }
      const graph = el('div', { class: 'panel' }, el('div', { class: 'timeline-ruler' },
        ...Array.from({ length: 5 }, (_, index) => el('span', {}, duration((current.start_us - origin + (current.end_us - current.start_us) * index / 4) * 1_000)))));
      const groups = new Map<string, Observation[]>();
      let unmapped = data.unaligned_count ?? 0;
      for (const record of data.records) {
        if (record.attributes?.clock_alignment === 'unmapped') { if (!data.unaligned_count) unmapped++; continue; }
        if (record.kind === 'relation') continue;
        const namespace = typeof record.attributes?.thread_id_source === 'string' ? record.attributes.thread_id_source : `unmapped · producer ${record.producer_id}`;
        const key = `${record.pid} / ${record.tid}\n${namespace}`;
        groups.set(key, [...(groups.get(key) ?? []), record]);
      }
      for (const [name, records] of groups) {
        const lane = el('div', { class: 'track-lane' });
        const ends: number[] = [];
        for (const record of records.sort((a, b) => a.timestamp_us - b.timestamp_us)) {
          const start = Math.max(0, (record.timestamp_us - current.start_us) / Math.max(1, current.end_us - current.start_us) * 100);
          const width = Math.min(100 - start, Math.max(.35, (record.duration_us ?? 0) / Math.max(1, current.end_us - current.start_us) * 100));
          let row = ends.findIndex(end => end <= start);
          if (row < 0) row = ends.length;
          ends[row] = start + width;
          const label = observationLabel(record);
          const mark = button(label, () => inspect(record), `timeline-event ${record.kind === 'context' || record.kind === 'span' ? 'context' : ''}`);
          mark.title = `${label} · ${record.kind} · ${record.basis}`;
          mark.style.left = `${start}%`;
          mark.style.width = `${width}%`;
          mark.style.top = `${8 + row * 27}px`;
          lane.append(mark);
        }
        lane.style.minHeight = `${Math.max(1, ends.length) * 27 + 16}px`;
        graph.append(el('div', { class: 'timeline-track' }, el('div', { class: 'track-label mono' }, `PID / thread\n${name}`), lane));
      }
      const rows = el('tbody', {}, ...data.records.map(record => el('tr', {},
        el('td', {}, button(observationLabel(record), () => inspect(record), 'text-button mono'), isModuleLifetime(record) ? el('small', { class: 'source-label' }, 'Observed mapping bounds') : null),
        el('td', {}, badge(record.kind)), el('td', {}, badge(record.basis)),
        el('td', { class: 'numeric' }, unmappedView || record.attributes?.clock_alignment === 'unmapped' ? 'Unmapped clock' : duration((record.timestamp_us - origin) * 1_000)),
        el('td', { class: 'numeric' }, record.duration_us === undefined ? '—' : duration(record.duration_us * 1_000)))));
      const next = data.next_cursor ? button('Next page', () => { cursor = data.next_cursor ?? undefined; void load(); }, 'button small') : null;
      canvas.replaceChildren(el('div', { class: 'stack' }, unmapped ? el('p', { class: 'notice' }, `${unmapped} observations have unmapped clocks and are omitted from the aligned plot. Select unmapped clock evidence to inspect those records.`) : null, !unmappedView ? graph : null, el('section', { class: 'panel' }, el('div', { class: 'table-scroll' }, el('table', {},
        el('thead', {}, el('tr', {}, el('th', {}, 'Observation'), el('th', {}, 'Kind'), el('th', {}, 'Basis'), el('th', { class: 'numeric' }, 'Offset'), el('th', { class: 'numeric' }, 'Duration'))), rows)),
        el('div', { class: 'table-foot' }, `${numeric(data.records.length)} observations on this page`, next))));
    } catch (error) { if (version === epoch) canvas.replaceChildren(errorView(error, () => { void load(); })); }
  }
  alignment.addEventListener('change', () => { cursor = undefined; window = undefined; void load(); });
  await load();
  return body;
}
