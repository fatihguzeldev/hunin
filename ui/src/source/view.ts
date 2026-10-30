import { runSource } from '../run';
import { operation, type Run } from '../api';
import { badge, el } from '../dom';
import type { SourceSelection } from '../investigation';
import { sourceNavigation } from './navigation';

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
  );
  const measuredScript =
    data.provenance &&
    typeof data.provenance === 'object' &&
    'source_role' in data.provenance &&
    data.provenance.source_role === 'runtime_observed_generated_script';
  const code = el(
    'pre',
    { class: 'source-code', tabindex: 0, 'aria-label': `Source ${data.path}` },
    ...(data.content === '' &&
    data.total_lines !== undefined &&
    data.start_line > data.total_lines
      ? []
      : data.content.split('\n')
    ).map((text, index) =>
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
    { class: 'stack' },
    el(
      'div',
      { class: 'view-title' },
      el('h2', { class: 'mono' }, data.path),
      badge(
        data.identity_verified
          ? 'source bytes verified'
          : 'source bytes unverified',
        data.identity_verified ? 'good' : 'warn',
      ),
      data.source_kind
        ? badge(
            measuredScript
              ? 'recorded runtime script'
              : data.source_kind.replaceAll('_', ' '),
          )
        : null,
      data.build_mapping_verified === false
        ? badge('build mapping unverified', 'warn')
        : null,
    ),
    data.diagnostic
      ? el('p', { class: 'notice' }, data.diagnostic)
      : !data.identity_verified
        ? el(
            'p',
            { class: 'notice' },
            'This source has not been verified against the measured build. Treat the location as a hint; the current file may differ from the recorded code.',
          )
        : data.build_mapping_verified !== true
          ? el(
              'p',
              { class: 'notice' },
              'The source bytes are verified. Their relationship to the loaded code has not been verified.',
            )
          : null,
    sourceNavigation(data, line, page),
    el(
      'section',
      { class: 'panel' },
      el('div', { class: 'table-scroll' }, code),
    ),
    data.provenance
      ? el(
          'details',
          { class: 'panel' },
          el('summary', { class: 'panel-body' }, 'Source mapping provenance'),
          el(
            'pre',
            { class: 'panel-body' },
            JSON.stringify(data.provenance, null, 2),
          ),
        )
      : null,
    codeId
      ? el(
          'p',
          { class: 'metric-note mono' },
          `Measured code identity ${codeId}`,
        )
      : null,
    el(
      'p',
      { class: 'metric-note' },
      `Run ${run.id} · source fingerprint ${runSource(run).fingerprint}`,
    ),
  );
}
