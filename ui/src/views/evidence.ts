import { runSource } from '../recordings/presentation';
import { operation, type Artifact } from '../api';
import { badge, button, el, empty, errorView, loading } from '../dom';
import { bytes, numeric } from '../format';
import type { Observation } from '../record.generated';
import type { ViewContext } from '../investigation/view';
import { hasInspectableSource } from '../source/paths';
import { canReanalyze } from '../recordings/formats';
import { observationLabel, referenceIds } from '../observation';
import { nativeEvidence } from '../native-evidence';
import { pagedList } from '../paged-list';
import { linkedRequest } from '../requests';

export async function evidence(context: ViewContext): Promise<HTMLElement> {
  const state = context.investigation.evidence;
  const cursor = state.cursor;
  let selectionRequest: AbortController | undefined;
  const [relationships, formats] = await Promise.all([
    operation(
      'contexts.query',
      {
        run_id: context.run.id,
        cursor,
        limit: 100,
      },
      context.signal,
    ),
    context
      .importFormats(context.signal)
      .then((catalog) => ({ catalog, error: undefined as unknown }))
      .catch((error: unknown) => ({ catalog: undefined, error })),
  ]);
  context.signal.throwIfAborted();
  const body = el('div', { class: 'stack' });
  if (
    !formats.catalog ||
    formats.catalog.some((format) => format.input_kind === undefined)
  ) {
    body.append(
      el(
        'p',
        { class: 'notice' },
        'Profile reanalysis is unavailable for formats this service has not identified as retained profiles.',
      ),
    );
  }
  if (formats.error) {
    body.append(
      el(
        'div',
        { class: 'notice' },
        `Recording format discovery failed. ${formats.error instanceof Error ? formats.error.message : String(formats.error)}`,
        button(
          'Retry format discovery',
          () => context.refresh(),
          'button small',
        ),
      ),
    );
  }
  const inspector = el('div');
  const names = new Map(
    relationships.contexts.map((record) => [
      record.id,
      record.name ?? record.id,
    ]),
  );
  const moduleIds = [
    ...new Set(
      relationships.relations
        .filter((record) => record.relation === 'sampled_in_code_instance')
        .map((record) => record.to)
        .filter((id): id is string => typeof id === 'string'),
    ),
  ].slice(0, 100);
  if (moduleIds.length) {
    const modules = await operation(
      'evidence.get',
      {
        run_id: context.run.id,
        ids: moduleIds,
      },
      context.signal,
    );
    context.signal.throwIfAborted();
    modules.records.forEach((record) =>
      names.set(record.id, observationLabel(record)),
    );
  }
  async function resolve(id: string, scroll = true): Promise<void> {
    selectionRequest?.abort();
    selectionRequest = new AbortController();
    const linked = linkedRequest(context.signal, selectionRequest.signal);
    try {
      const data = await operation(
        'evidence.get',
        { run_id: context.run.id, ids: [id] },
        linked.signal,
      );
      if (linked.signal.aborted) return;
      if (data.records[0]) inspect(data.records[0], scroll);
      else {
        state.selection = undefined;
        context.persist();
        inspector.replaceChildren(
          el(
            'p',
            { class: 'notice' },
            `Evidence ${id} is unavailable in the current selected results.`,
          ),
        );
      }
    } catch (error) {
      if (!linked.signal.aborted)
        inspector.replaceChildren(
          errorView(error, () => {
            void resolve(id, scroll);
          }),
        );
    } finally {
      linked.finish();
    }
  }
  function inspect(record: Observation, scroll = true): void {
    if (context.signal.aborted) return;
    selectionRequest?.abort();
    state.selection = { kind: 'observation', id: record.id };
    context.persist();
    const references = referenceIds(record);
    const frame = record.stack?.find((value) =>
      hasInspectableSource(value.file, value.code_id),
    );
    const native = nativeEvidence(record);
    inspector.replaceChildren(
      el(
        'section',
        { class: 'panel' },
        el(
          'div',
          { class: 'panel-head' },
          el('h2', {}, observationLabel(record)),
          badge(record.basis),
        ),
        frame
          ? el(
              'div',
              { class: 'panel-body' },
              button(
                `Open ${frame.file}:${frame.line ?? ''}`,
                () => context.source(frame.file!, frame.line, frame.code_id),
                'text-button mono',
              ),
            )
          : null,
        native ? el('div', { class: 'panel-body' }, native) : null,
        el('pre', { class: 'panel-body' }, JSON.stringify(record, null, 2)),
        references.length
          ? el(
              'div',
              { class: 'panel-body toolbar' },
              ...references.map((id) =>
                button(
                  `Open evidence ${id}`,
                  () => resolve(id),
                  'button small mono',
                ),
              ),
            )
          : null,
      ),
    );
    if (scroll)
      inspector.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }
  function reanalyze(id: string, action: HTMLButtonElement): void {
    action.disabled = true;
    inspector.replaceChildren(loading('Reanalyzing retained profile inputs…'));
    void operation('recordings.reanalyze', {
      run_id: context.run.id,
      artifact_id: id,
    })
      .then((result) => {
        context.refresh(
          [
            result.accepted
              ? 'Normalization updated.'
              : 'Normalization unchanged.',
            ...result.diagnostics,
          ].join(' '),
        );
      })
      .catch((error) => {
        if (context.signal.aborted) return;
        action.disabled = false;
        inspector.replaceChildren(
          errorView(error, () => reanalyze(id, action)),
        );
      });
  }
  async function inspectArtifact(
    id: string,
    name: string,
    offset = 0,
    scroll = true,
  ): Promise<void> {
    if (context.signal.aborted) return;
    selectionRequest?.abort();
    selectionRequest = new AbortController();
    const linked = linkedRequest(context.signal, selectionRequest.signal);
    try {
      const result = await operation(
        'artifacts.read',
        {
          run_id: context.run.id,
          artifact_id: id,
          offset,
          limit: 5000,
        },
        linked.signal,
      );
      if (linked.signal.aborted) return;
      state.selection = { kind: 'artifact', id, name, offset };
      context.persist();
      inspector.replaceChildren(
        el(
          'section',
          { class: 'panel' },
          el(
            'div',
            { class: 'panel-head' },
            el('h2', { class: 'mono' }, name),
            el(
              'small',
              {},
              `UTF-8 preview · offset ${offset} · ${bytes(result.size)}`,
            ),
          ),
          el(
            'p',
            { class: 'panel-body metric-note' },
            'Non-text bytes may be replaced in this preview. The retained artifact remains unchanged.',
          ),
          el('pre', { class: 'panel-body' }, result.content),
          result.next_offset != null
            ? el(
                'div',
                { class: 'panel-body' },
                button(
                  'Next artifact chunk',
                  () => {
                    void inspectArtifact(id, name, result.next_offset!);
                  },
                  'button small',
                ),
              )
            : null,
        ),
      );
      if (scroll) inspector.scrollIntoView({ block: 'nearest' });
    } catch (error) {
      if (linked.signal.aborted) return;
      inspector.replaceChildren(
        errorView(error, () => {
          void inspectArtifact(id, name, offset, scroll);
        }),
      );
    } finally {
      linked.finish();
    }
  }
  const contexts = el(
    'section',
    { class: 'panel' },
    el(
      'div',
      { class: 'panel-head' },
      el('h2', {}, 'Execution contexts'),
      el('small', {}, numeric(relationships.contexts.length)),
    ),
  );
  if (!relationships.contexts.length)
    contexts.append(
      empty(
        'No explicit contexts',
        'SDK contexts or accepted analyzer output will appear here when available.',
      ),
    );
  else
    contexts.append(
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
              el('th', {}, 'Context'),
              el('th', {}, 'Process / thread'),
              el('th', {}, 'Basis'),
            ),
          ),
          el(
            'tbody',
            {},
            ...relationships.contexts.map((record) =>
              el(
                'tr',
                {},
                el(
                  'td',
                  {},
                  button(
                    record.name ?? record.id,
                    () => inspect(record),
                    'text-button mono',
                  ),
                  el('small', { class: 'source-label mono' }, record.id),
                ),
                el('td', { class: 'mono' }, `${record.pid} / ${record.tid}`),
                el('td', {}, badge(record.basis)),
              ),
            ),
          ),
        ),
      ),
    );
  const relations = el(
    'section',
    { class: 'panel' },
    el(
      'div',
      { class: 'panel-head' },
      el('h2', {}, 'Recorded relationships'),
      el('small', {}, 'Graph edges retain their evidence'),
    ),
  );
  if (!relationships.relations.length)
    relations.append(
      empty(
        'No accepted relationships',
        'Concurrent work is kept separate until an SDK or analyzer supplies a supported relationship.',
      ),
    );
  else
    for (const record of relationships.relations)
      relations.append(
        el(
          'div',
          { class: 'relation' },
          el(
            'div',
            {},
            record.from
              ? button(
                  names.get(record.from) ?? record.from,
                  () => resolve(record.from!),
                  'text-button mono',
                )
              : 'Unknown origin',
          ),
          el(
            'div',
            {},
            button(
              record.relation ?? 'relation',
              () => inspect(record),
              'text-button',
            ),
            el(
              'p',
              { class: 'relation-basis' },
              record.basis,
              ' · ',
              `${record.evidence?.length ?? 0} evidence references`,
            ),
          ),
          el(
            'div',
            {},
            record.to
              ? button(
                  names.get(record.to) ?? record.to,
                  () => resolve(record.to!),
                  'text-button mono',
                )
              : 'Unknown destination',
          ),
        ),
      );
  const artifacts = pagedList<Artifact>({
    label: 'artifacts',
    signal: context.signal,
    location: state.artifacts,
    onLocation: (location, navigate) => {
      state.artifacts = location;
      context.persist(navigate ? 'push' : 'replace');
    },
    nextLabel: 'More artifacts',
    load: async (pageCursor, readSignal) => {
      const page = await operation(
        'artifacts.list',
        {
          run_id: context.run.id,
          cursor: pageCursor,
          limit: 50,
        },
        readSignal,
      );
      return { items: page.artifacts, next_cursor: page.next_cursor };
    },
    render: (items) =>
      el(
        'section',
        { class: 'panel' },
        el('div', { class: 'panel-head' }, el('h2', {}, 'Immutable artifacts')),
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
                el('th', {}, 'Artifact'),
                el('th', {}, 'Format'),
                el('th', { class: 'numeric' }, 'Size'),
                el('th', {}, 'SHA-256'),
                el('th', {}, 'Normalization'),
              ),
            ),
            el(
              'tbody',
              {},
              ...items.map((item) => {
                const action = button(
                  'Reanalyze',
                  () => reanalyze(item.id, action),
                  'button small',
                );
                action.setAttribute('aria-label', `Reanalyze ${item.name}`);
                return el(
                  'tr',
                  {},
                  el(
                    'td',
                    {},
                    button(
                      item.name,
                      () => {
                        void inspectArtifact(item.id, item.name);
                      },
                      'text-button mono',
                    ),
                  ),
                  el('td', {}, item.format),
                  el('td', { class: 'numeric' }, bytes(item.size)),
                  el(
                    'td',
                    { class: 'mono', title: item.sha256 },
                    item.sha256.slice(0, 16),
                  ),
                  el(
                    'td',
                    {},
                    canReanalyze(item.format, formats.catalog) ? action : '—',
                  ),
                );
              }),
            ),
          ),
        ),
      ),
  });
  body.append(
    el(
      'details',
      { class: 'panel' },
      el(
        'summary',
        { class: 'panel-body' },
        'Measured source and executable identities',
      ),
      runSource(context.run).build_source_verified !== true
        ? el(
            'p',
            { class: 'panel-body metric-note' },
            'Source and executable identities are distinct. Their build relationship has not been verified.',
          )
        : null,
      el(
        'pre',
        { class: 'panel-body' },
        JSON.stringify(context.run.source, null, 2),
      ),
    ),
    contexts,
    relations,
    inspector,
    artifacts.element,
  );
  function showPage(page?: string): void {
    state.cursor = page;
    state.selection = undefined;
    context.persist('push');
    context.refresh();
  }
  if (relationships.next_cursor)
    body.append(
      button('Next context / relationship page', () => {
        void showPage(relationships.next_cursor ?? undefined);
      }),
    );
  if (cursor)
    body.append(
      button(
        'First evidence page',
        () => {
          void showPage();
        },
        'button subtle',
      ),
    );
  await artifacts.refresh();
  if (!context.signal.aborted && state.selection) {
    const selected = state.selection;
    if (selected.kind === 'observation') await resolve(selected.id, false);
    else
      await inspectArtifact(selected.id, selected.name, selected.offset, false);
  }
  return body;
}
