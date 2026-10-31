import { operation } from '../api';
import { badge, button, el, empty, errorView, field, loading } from '../dom';
import type { ViewContext } from '../investigation/view';
import { observationLabel, referenceIds } from '../observation';
import { linkedRequest } from '../requests';
import { outputArtifact, outputSelections } from './output';

/** Accepted stdout remains retained evidence; current observations resolve by exact ID. */
export function analyzerOutput(
  context: ViewContext,
  attempt: unknown,
): HTMLElement {
  const artifact = outputArtifact(attempt);
  const body = el('div', { class: 'stack' });
  const root = el(
    'details',
    {},
    el('summary', {}, 'Inspect accepted output'),
    body,
  );
  if (!artifact) {
    body.append(
      empty(
        'Output artifact unavailable',
        'This attempt has no retained stdout reference. Its metadata remains in Attempt details.',
      ),
    );
    return root;
  }
  const preview = el('div', { class: 'stack' });
  const inspector = el('div', { class: 'stack', 'aria-live': 'polite' });
  const id = el('input', {
    placeholder: 'Exact observation ID',
    autocomplete: 'off',
  });
  const lookup = el(
    'form',
    { class: 'toolbar' },
    field('Observation ID', id),
    el(
      'button',
      { type: 'submit', class: 'button subtle' },
      'Inspect observation',
    ),
  );
  lookup.addEventListener('submit', (event) => {
    event.preventDefault();
    if (id.value.trim()) void inspect(id.value.trim());
  });
  body.append(preview, lookup, inspector);
  let opened = false;
  let pageRequest: AbortController | undefined;
  let selectionRequest: AbortController | undefined;
  let pageEpoch = 0;
  let selectionEpoch = 0;

  async function inspect(recordId: string): Promise<void> {
    selectionRequest?.abort();
    selectionRequest = new AbortController();
    const linked = linkedRequest(context.signal, selectionRequest.signal);
    const epoch = ++selectionEpoch;
    inspector.replaceChildren(loading('Loading observation…'));
    try {
      const result = await operation(
        'evidence.get',
        { run_id: context.run.id, ids: [recordId] },
        linked.signal,
      );
      if (linked.signal.aborted || epoch !== selectionEpoch) return;
      const record = result.records.find((row) => row.id === recordId);
      if (!record) {
        inspector.replaceChildren(
          empty(
            'Observation unavailable',
            'This ID is absent from the recording’s currently selected results. The retained attempt output remains available above.',
          ),
        );
        return;
      }
      id.value = record.id;
      const title = el('h3', { tabindex: -1 }, observationLabel(record));
      const references = referenceIds(record);
      const links = el('div', { class: 'toolbar' });
      let shown = 0;
      const more = button(
        'More evidence references',
        showReferences,
        'button subtle small',
      );
      function showReferences(): void {
        more.remove();
        for (const reference of references.slice(shown, shown + 25))
          links.append(
            button(
              `Inspect ${reference}`,
              () => {
                void inspect(reference);
              },
              'button subtle small mono',
            ),
          );
        shown += 25;
        if (shown < references.length) links.append(more);
      }
      showReferences();
      inspector.replaceChildren(
        el(
          'div',
          { class: 'stack' },
          title,
          el(
            'div',
            { class: 'toolbar' },
            badge(record.kind),
            badge(record.basis),
            el('span', { class: 'metric-note' }, `PID ${record.pid}`),
          ),
          references.length
            ? el(
                'p',
                { class: 'metric-note' },
                `${references.length} evidence references`,
              )
            : null,
          links,
          el(
            'details',
            {},
            el('summary', {}, 'Observation details'),
            el('pre', {}, JSON.stringify(record, null, 2)),
          ),
        ),
      );
      title.focus({ preventScroll: true });
    } catch (error) {
      if (!linked.signal.aborted && epoch === selectionEpoch)
        inspector.replaceChildren(
          errorView(error, () => {
            void inspect(recordId);
          }),
        );
    } finally {
      linked.finish();
    }
  }

  async function load(offset = 0, previous: number[] = []): Promise<void> {
    pageRequest?.abort();
    selectionRequest?.abort();
    pageRequest = new AbortController();
    const linked = linkedRequest(context.signal, pageRequest.signal);
    const epoch = ++pageEpoch;
    const selectionAtStart = selectionEpoch;
    preview.replaceChildren(loading('Loading retained output…'));
    inspector.replaceChildren();
    try {
      // One preceding byte distinguishes a line boundary from a partial JSONL row.
      const start = Math.max(0, offset - 1);
      const result = await operation(
        'artifacts.read',
        {
          run_id: context.run.id,
          artifact_id: artifact!.id,
          offset: start,
          limit: 5000,
        },
        linked.signal,
      );
      if (linked.signal.aborted || epoch !== pageEpoch) return;
      const selections = outputSelections(
        result.content,
        context.run.id,
        offset > 0,
        result.next_offset === null,
      );
      preview.replaceChildren(
        el(
          'p',
          { class: 'metric-note' },
          `Retained output · bytes ${start}–${result.next_offset ?? result.size} of ${result.size}. Inspect a record to resolve its current observation and evidence.`,
        ),
        selections.length
          ? el(
              'div',
              { class: 'toolbar' },
              ...selections.map((record) =>
                button(
                  record.label,
                  () => {
                    void inspect(record.id);
                  },
                  'button subtle',
                ),
              ),
            )
          : empty(
              result.size
                ? 'No complete observations in this excerpt'
                : 'No output observations',
              result.size
                ? result.next_offset !== null
                  ? 'This excerpt may contain only part of a record. Continue reading the output, or enter an exact observation ID below.'
                  : offset > 0
                    ? 'This excerpt may begin inside a record. Enter its exact observation ID below, or return to the previous excerpt.'
                    : 'No derived observation IDs could be read. The retained output is available below.'
                : 'The analyzer accepted an empty output.',
            ),
        el(
          'details',
          {},
          el('summary', {}, 'Raw output excerpt'),
          el(
            'p',
            { class: 'metric-note' },
            'UTF-8 preview; boundary bytes may be replaced. Only complete JSON lines above are selectable.',
          ),
          el('pre', {}, result.content),
        ),
      );
      const controls = el('div', { class: 'toolbar' });
      if (previous.length)
        controls.append(
          button(
            'Previous output excerpt',
            () => {
              void load(previous.at(-1)!, previous.slice(0, -1));
            },
            'button subtle',
          ),
        );
      const next = result.next_offset;
      if (next !== null && next > offset)
        controls.append(
          button(
            'Next output excerpt',
            () => {
              void load(next, [...previous, offset]);
            },
            'button subtle',
          ),
        );
      preview.append(controls);
      if (selections[0] && selectionEpoch === selectionAtStart)
        void inspect(selections[0].id);
    } catch (error) {
      if (!linked.signal.aborted && epoch === pageEpoch)
        preview.replaceChildren(
          errorView(error, () => {
            void load(offset, previous);
          }),
        );
    } finally {
      linked.finish();
    }
  }
  root.addEventListener('toggle', () => {
    if (root.open && !opened) {
      opened = true;
      void load();
    }
  });
  return root;
}
