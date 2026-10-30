import type { OperationOutputs } from '../operations.generated';
import { countLabel, countStatus } from '../call-counts';
import { badge, button, el, empty, loading } from '../dom';
type CountReport = OperationOutputs['calls.query'];

function recordingLink(id: string, text: string): HTMLElement {
  return el(
    'a',
    { href: `?run=${encodeURIComponent(id)}&view=calls`, class: 'mono' },
    `${text}: ${id}`,
  );
}
export function countReport(
  data: CountReport,
  actions: {
    cancel: () => Promise<void>;
    refresh: () => void;
    more: (cursor: string) => void;
  },
): HTMLElement {
  const content = el(
    'section',
    { class: 'panel panel-body stack' },
    el(
      'div',
      { class: 'toolbar' },
      el('h2', {}, 'Function call counts'),
      badge(countStatus(data.status)),
    ),
    el(
      'div',
      {},
      recordingLink(
        data.run_id,
        data.parent_run_id ? 'Counting recording' : 'Recording',
      ),
    ),
    data.parent_run_id
      ? el('div', {}, recordingLink(data.parent_run_id, 'Original recording'))
      : null,
    el(
      'p',
      { class: 'metric-note' },
      data.parent_run_id
        ? 'Window: this separate counting execution. These counts were not collected during the original sampled execution.'
        : data.status === 'not_collected'
          ? 'A new counting execution is required; sampled stack weights do not establish function invocation counts.'
          : 'Window: this counting recording describes its own execution.',
    ),
    el(
      'p',
      {},
      `Method: ${data.method ?? 'Not collected'} · requested ${data.requested_scope} · collection ${data.collection_scope.replaceAll('_', ' ')}`,
    ),
    ...data.diagnostics.map((text) => el('p', { class: 'notice' }, text)),
    ...data.limitations.map((text) => el('p', { class: 'metric-note' }, text)),
  );
  if (data.status === 'running') {
    const cancel = button('Cancel counting run', () => {
      cancel.disabled = true;
      void actions.cancel().catch(() => {
        cancel.disabled = false;
      });
    });
    content.append(loading('Counting in the separate recording…'), cancel);
  }
  if (data.counts.length) {
    content.append(
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
              el('th', { class: 'numeric' }, 'Calls'),
              el('th', { class: 'numeric' }, 'Resumes'),
            ),
          ),
          el(
            'tbody',
            {},
            ...data.counts.map((row) =>
              el(
                'tr',
                {},
                el(
                  'td',
                  {},
                  el('span', { class: 'mono' }, row.function.name),
                  el(
                    'small',
                    { class: 'source-label mono' },
                    `${row.function.file ?? 'Source unavailable'}${row.function.line == null ? '' : `:${row.function.line}`}`,
                  ),
                  'code_id' in row.function &&
                    typeof row.function.code_id === 'string'
                    ? el(
                        'small',
                        {
                          class: 'source-label mono',
                          title: 'Code instance identity',
                        },
                        row.function.code_id,
                      )
                    : null,
                ),
                el('td', { class: 'numeric' }, countLabel(row.calls)),
                el('td', { class: 'numeric' }, countLabel(row.resumes)),
              ),
            ),
          ),
        ),
      ),
    );
    content.append(
      el(
        'p',
        { class: 'metric-note' },
        `${data.counts.length.toLocaleString()} displayed · ${data.total_count.toLocaleString()} retained functions. Missing functions are not measured zeros.`,
      ),
    );
  } else if (data.status !== 'running')
    content.append(
      empty(
        countStatus(data.status),
        data.status === 'not_collected'
          ? 'Choose all functions or select functions, then measure calls in a new recording.'
          : 'No function counts were retained for this scope. Inspect the diagnostics and limitations.',
      ),
    );
  if (data.next_cursor)
    content.append(
      button(
        'More function counts',
        () => {
          actions.more(data.next_cursor!);
        },
        'button subtle',
      ),
    );
  content.append(
    button(
      'Refresh counts',
      () => {
        actions.refresh();
      },
      'button subtle',
    ),
  );
  return content;
}
