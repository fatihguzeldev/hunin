import { runSource } from '../run';
import { operation, type Run, type Artifact } from '../api';
import { badge, button, el, empty, errorView, loading } from '../dom';
import { bytes, numeric } from '../format';
import type { Observation } from '../record.generated';
import type { ViewContext } from '../workspace';
import { hasInspectableSource } from '../source-path';
import { canReanalyze } from '../artifact';
import { observationLabel, referenceIds } from '../observation';
import { nativeEvidence } from '../native-evidence';
import { pagedList } from '../paged-list';

export async function source(run: Run, path: string, line?: number, codeId?: string): Promise<HTMLElement> {
  const data = await operation('source.read', { run_id: run.id, path, line, code_id: codeId, count: 120 });
  const measuredScript = data.provenance && typeof data.provenance === 'object' && 'source_role' in data.provenance && data.provenance.source_role === 'runtime_observed_generated_script';
  const code = el('pre', { class: 'source-code', tabindex: 0, 'aria-label': `Source ${data.path}` },
    ...data.content.split('\n').map((text, index) => el('span', { class: `source-line ${data.start_line + index === line ? 'selected' : ''}`, 'data-line': data.start_line + index }, text || ' ')));
  return el('div', { class: 'stack' },
    el('div', { class: 'view-title' }, el('h2', { class: 'mono' }, data.path), badge(data.identity_verified ? 'source bytes verified' : 'source bytes unverified', data.identity_verified ? 'good' : 'warn'),
      data.source_kind ? badge(measuredScript ? 'recorded runtime script' : data.source_kind.replaceAll('_', ' ')) : null,
      data.build_mapping_verified === false ? badge('build mapping unverified', 'warn') : null),
    data.diagnostic ? el('p', { class: 'notice' }, data.diagnostic) : !data.identity_verified ? el('p', { class: 'notice' }, 'This source has not been verified against the measured build. Treat the location as a hint; the current file may differ from the recorded code.') :
      data.build_mapping_verified !== true ? el('p', { class: 'notice' }, 'The source bytes are verified. Their relationship to the loaded code has not been verified.') : null,
    el('section', { class: 'panel' }, el('div', { class: 'table-scroll' }, code)),
    data.provenance ? el('details', { class: 'panel' }, el('summary', { class: 'panel-body' }, 'Source mapping provenance'), el('pre', { class: 'panel-body' }, JSON.stringify(data.provenance, null, 2))) : null,
    codeId ? el('p', { class: 'metric-note mono' }, `Measured code identity ${codeId}`) : null,
    el('p', { class: 'metric-note' }, `Run ${run.id} · source fingerprint ${runSource(run).fingerprint}`));
}

export async function evidence(context: ViewContext, cursor?: string): Promise<HTMLElement> {
  const relationships = await operation('contexts.query', { run_id: context.run.id, cursor, limit: 100 });
  const body = el('div', { class: 'stack' });
  const inspector = el('div');
  const names = new Map(relationships.contexts.map(record => [record.id, record.name ?? record.id]));
  const moduleIds = [...new Set(relationships.relations.filter(record => record.relation === 'sampled_in_code_instance').map(record => record.to).filter((id): id is string => typeof id === 'string'))].slice(0, 100);
  if (moduleIds.length) {
    const modules = await operation('evidence.get', { run_id: context.run.id, ids: moduleIds });
    modules.records.forEach(record => names.set(record.id, observationLabel(record)));
  }
  function resolve(id: string): void {
    void operation('evidence.get', { run_id: context.run.id, ids: [id] }).then(data => {
      if (!data.records.length) inspector.append(el('p', { class: 'notice' }, `Evidence ${id} is unavailable in this recording.`));
      else data.records.forEach(value => inspect(value));
    }).catch(error => inspector.append(errorView(error, () => resolve(id))));
  }
  function inspect(record: Observation): void {
    const references = referenceIds(record);
    const frame = record.stack?.find(value => hasInspectableSource(value.file, value.code_id));
    const native = nativeEvidence(record);
    inspector.replaceChildren(el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, observationLabel(record)), badge(record.basis)),
      frame ? el('div', { class: 'panel-body' }, button(`Open ${frame.file}:${frame.line ?? ''}`, () => context.source(frame.file!, frame.line, frame.code_id), 'text-button mono')) : null,
      native ? el('div', { class: 'panel-body' }, native) : null,
      el('pre', { class: 'panel-body' }, JSON.stringify(record, null, 2)),
      references.length ? el('div', { class: 'panel-body toolbar' }, ...references.map(id => button(`Open evidence ${id}`, () => resolve(id), 'button small mono'))) : null));
    inspector.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }
  function reanalyze(id: string, action: HTMLButtonElement): void {
    action.disabled = true;
    inspector.replaceChildren(loading('Reanalyzing retained profile inputs…'));
    void operation('recordings.reanalyze', { run_id: context.run.id, artifact_id: id }).then(async result => {
      const refreshed = await evidence(context, cursor);
      refreshed.prepend(el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, result.accepted ? 'Normalization updated' : 'Normalization unchanged')),
        el('div', { class: 'panel-body stack' }, el('p', {}, 'Evidence and artifacts refreshed from accepted storage. Other recording views reload when selected.'),
          ...result.diagnostics.map(message => el('p', { class: 'notice' }, message)),
          el('details', {}, el('summary', {}, 'Accepted recording summary'), el('pre', {}, JSON.stringify(result.summary, null, 2))))));
      body.replaceWith(refreshed);
    }).catch(error => { action.disabled = false; inspector.replaceChildren(errorView(error, () => reanalyze(id, action))); });
  }
  async function inspectArtifact(id: string, name: string, offset = 0): Promise<void> {
    try {
      const result = await operation('artifacts.read', { run_id: context.run.id, artifact_id: id, offset, limit: 5000 });
      inspector.replaceChildren(el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', { class: 'mono' }, name), el('small', {}, `UTF-8 preview · offset ${offset} · ${bytes(result.size)}`)),
        el('p', { class: 'panel-body metric-note' }, 'Non-text bytes may be replaced in this preview. The retained artifact remains unchanged.'), el('pre', { class: 'panel-body' }, result.content),
        result.next_offset != null ? el('div', { class: 'panel-body' }, button('Next artifact chunk', () => { void inspectArtifact(id, name, result.next_offset!); }, 'button small')) : null));
      inspector.scrollIntoView({ block: 'nearest' });
    } catch (error) { inspector.replaceChildren(errorView(error, () => { void inspectArtifact(id, name, offset); })); }
  }
  const contexts = el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Execution contexts'), el('small', {}, numeric(relationships.contexts.length))));
  if (!relationships.contexts.length) contexts.append(empty('No explicit contexts', 'SDK contexts or accepted analyzer output will appear here when available.'));
  else contexts.append(el('div', { class: 'table-scroll' }, el('table', {},
    el('thead', {}, el('tr', {}, el('th', {}, 'Context'), el('th', {}, 'Process / thread'), el('th', {}, 'Basis'))),
    el('tbody', {}, ...relationships.contexts.map(record => el('tr', {},
      el('td', {}, button(record.name ?? record.id, () => inspect(record), 'text-button mono'), el('small', { class: 'source-label mono' }, record.id)),
      el('td', { class: 'mono' }, `${record.pid} / ${record.tid}`), el('td', {}, badge(record.basis))))))));
  const relations = el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Recorded relationships'), el('small', {}, 'Graph edges retain their evidence')));
  if (!relationships.relations.length) relations.append(empty('No accepted relationships', 'Concurrent work is kept separate until an SDK or analyzer supplies a supported relationship.'));
  else for (const record of relationships.relations) relations.append(el('div', { class: 'relation' },
    el('div', {}, record.from ? button(names.get(record.from) ?? record.from, () => resolve(record.from!), 'text-button mono') : 'Unknown origin'),
    el('div', {}, button(record.relation ?? 'relation', () => inspect(record), 'text-button'),
      el('p', { class: 'relation-basis' }, record.basis, ' · ', `${record.evidence?.length ?? 0} evidence references`)),
    el('div', {}, record.to ? button(names.get(record.to) ?? record.to, () => resolve(record.to!), 'text-button mono') : 'Unknown destination')));
  const artifacts = pagedList<Artifact>({
    label: 'artifacts', nextLabel: 'More artifacts',
    load: async pageCursor => { const page = await operation('artifacts.list', { run_id: context.run.id, cursor: pageCursor, limit: 50 }); return { items: page.artifacts, next_cursor: page.next_cursor }; },
    render: items => el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Immutable artifacts')),
    el('div', { class: 'table-scroll' }, el('table', {}, el('thead', {}, el('tr', {}, el('th', {}, 'Artifact'), el('th', {}, 'Format'), el('th', { class: 'numeric' }, 'Size'), el('th', {}, 'SHA-256'), el('th', {}, 'Normalization'))),
      el('tbody', {}, ...items.map(item => {
        const action = button('Reanalyze', () => reanalyze(item.id, action), 'button small');
        action.setAttribute('aria-label', `Reanalyze ${item.name}`);
        return el('tr', {}, el('td', {}, button(item.name, () => { void inspectArtifact(item.id, item.name); }, 'text-button mono')), el('td', {}, item.format), el('td', { class: 'numeric' }, bytes(item.size)), el('td', { class: 'mono', title: item.sha256 }, item.sha256.slice(0, 16)),
          el('td', {}, canReanalyze(item.format) ? action : '—'));
      }))))),
  });
  body.append(el('details', { class: 'panel' }, el('summary', { class: 'panel-body' }, 'Measured source and executable identities'),
    runSource(context.run).build_source_verified !== true ? el('p', { class: 'panel-body metric-note' }, 'Source and executable identities are distinct. Their build relationship has not been verified.') : null,
    el('pre', { class: 'panel-body' }, JSON.stringify(context.run.source, null, 2))), contexts, relations, inspector, artifacts.element);
  async function showPage(page?: string): Promise<void> {
    try { body.replaceWith(await evidence(context, page)); }
    catch (error) { inspector.replaceChildren(errorView(error, () => { void showPage(page); })); }
  }
  if (relationships.next_cursor) body.append(button('Next context / relationship page', () => { void showPage(relationships.next_cursor); }));
  if (cursor) body.append(button('First evidence page', () => { void showPage(); }, 'button subtle'));
  await artifacts.refresh();
  return body;
}
