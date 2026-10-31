import { runSource } from '../recordings/presentation';
import { operation, type Run } from '../api';
import { badge, button, el, empty } from '../dom';
import type { SourceSelection } from '../investigation/state';
import { sourceNavigation } from './navigation';
import { sourcePresentation } from './presentation';
import { sourceUnavailable } from './errors';

export async function source(
  run: Run,
  selected: SourceSelection,
  signal: AbortSignal,
  page: (startLine: number) => void,
): Promise<HTMLElement> {
  const { path, line, codeId, startLine } = selected;
  const data = await operation(
    'source.read',
    {
      run_id: run.id,
      path,
      line: startLine ?? line,
      code_id: codeId,
      count: 120,
    },
    signal,
  ).catch((error: unknown) => {
    const unavailable = sourceUnavailable(error);
    if (!unavailable) throw error;
    return empty(unavailable.title, unavailable.message);
  });
  signal.throwIfAborted();
  if (data instanceof HTMLElement) return data;
  const presentation = sourcePresentation(data);
  const code = el(
    'pre',
    { class: 'source-code', tabindex: 0, 'aria-label': `Source ${data.path}` },
    ...(presentation.content === 'lines' ? data.content.split('\n') : []).map(
      (text, index) =>
        el(
          'span',
          {
            class: `source-line ${data.start_line + index === line ? 'selected' : ''}`,
            'data-line': data.start_line + index,
          },
          text || ' ',
        ),
    ),
  );
  return el(
    'div',
    { class: 'stack source-view' },
    el(
      'div',
      { class: 'view-title' },
      el(
        'h2',
        { class: 'mono', title: data.path },
        data.path.replaceAll('\\', '/').split('/').at(-1) || data.path,
      ),
      badge(presentation.origin),
      badge(presentation.identity, presentation.tone),
    ),
    el(
      'p',
      { class: data.identity_verified ? 'metric-note' : 'notice' },
      presentation.note,
    ),
    presentation.content !== 'empty-file'
      ? sourceNavigation(data, line, page)
      : null,
    presentation.content === 'lines'
      ? el(
          'section',
          { class: 'panel' },
          el('div', { class: 'table-scroll' }, code),
        )
      : presentation.content === 'empty-file'
        ? empty('Empty source file', 'This file contains no source lines.')
        : presentation.content === 'outside-window'
          ? empty(
              'No lines at this position',
              `This source has ${data.total_lines} lines.`,
              button('Go to first line', () => page(1)),
            )
          : empty(
              'No source text returned',
              'The source line count is unknown. Try another line above.',
            ),
    line !== undefined && presentation.content === 'lines'
      ? el(
          'p',
          { class: 'metric-note' },
          `Line ${line} is the recorded location. Per-line execution cost is not measured.`,
        )
      : null,
    el(
      'details',
      { class: 'panel' },
      el('summary', { class: 'panel-body' }, 'Source details'),
      el(
        'div',
        { class: 'panel-body stack' },
        el('p', { class: 'mono' }, data.path),
        el('p', {}, presentation.mapping),
        data.diagnostic ? el('p', {}, data.diagnostic) : null,
        codeId ? el('p', { class: 'mono' }, `Code identity ${codeId}`) : null,
        el('p', { class: 'mono' }, `Recording ${run.id}`),
        el(
          'p',
          { class: 'mono' },
          `Source fingerprint ${runSource(run).fingerprint}`,
        ),
        data.provenance
          ? el('pre', {}, JSON.stringify(data.provenance, null, 2))
          : null,
      ),
    ),
  );
}
