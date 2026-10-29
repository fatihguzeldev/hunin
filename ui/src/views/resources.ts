import { operation, type Series } from '../api';
import { button, el, empty, errorView, loading } from '../dom';
import { duration, numeric } from '../format';
import type { ViewContext } from '../workspace';

function plot(series: Series): SVGSVGElement {
  const points = series.points.slice(0, 2_000);
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 900 160');
  svg.setAttribute('role', 'img');
  svg.setAttribute(
    'aria-label',
    `${series.name}, ${series.unit}, ${series.scope} scope. Values are listed in the table below.`,
  );
  svg.style.width = '100%';
  svg.style.maxHeight = '180px';
  const xMin = Math.min(...points.map((point) => point.timestamp_us));
  const xMax = Math.max(...points.map((point) => point.timestamp_us));
  const yMin = Math.min(0, ...points.map((point) => point.value));
  const yMax = Math.max(yMin + 1, ...points.map((point) => point.value));
  const line = document.createElementNS(svg.namespaceURI, 'polyline');
  line.setAttribute(
    'points',
    [...points]
      .sort((a, b) => a.timestamp_us - b.timestamp_us)
      .map(
        (point) =>
          `${16 + ((point.timestamp_us - xMin) / Math.max(1, xMax - xMin)) * 868},${144 - ((point.value - yMin) / (yMax - yMin)) * 128}`,
      )
      .join(' '),
  );
  line.setAttribute('fill', 'none');
  line.setAttribute('stroke', '#e1b96a');
  line.setAttribute('stroke-width', '2');
  const title = document.createElementNS(svg.namespaceURI, 'title');
  title.textContent = `PID ${series.pid} · ${series.scope}`;
  line.append(title);
  svg.append(line);
  return svg;
}

function aligned(series: Series): boolean {
  return ['aligned', 'epoch'].includes(series.clock_alignment ?? '');
}

function identity(series: Series): string {
  return `PID ${series.pid} · thread ${series.tid || 'unavailable'} (${series.thread_id_source ?? 'namespace unknown'}) · context ${series.context_id ?? 'unavailable'} · producer ${series.producer_id ?? 'unavailable'} · clock ${series.clock ?? 'unknown'} (${series.clock_alignment ?? 'unknown'})${typeof series.process_role === 'string' && series.process_role ? ` · ${series.process_role}` : ''}`;
}

export async function resources(
  context: ViewContext,
  cursor?: string,
): Promise<HTMLElement> {
  const { series, next_cursor, returned_points, total_points } =
    await operation(
      'resources.query',
      {
        run_id: context.run.id,
        limit: 12,
        point_limit: 2000,
        cursor,
      },
      context.signal,
    );
  context.signal.throwIfAborted();
  if (!series.length)
    return empty(
      'No resource observations',
      'This recording has no accepted process or thread resource series.',
    );
  const body = el(
    'div',
    { class: 'stack' },
    el(
      'p',
      { class: 'notice' },
      'Resource scope is part of each measurement. A process total is shared by its work and cannot be assigned to a request from interval overlap.',
    ),
    ...series.map((item) =>
      el(
        'section',
        { class: 'panel' },
        el(
          'div',
          { class: 'panel-head' },
          el('h2', {}, item.name),
          el('small', {}, `${item.unit} · ${item.scope} scope`),
        ),
        el(
          'div',
          { class: 'panel-body stack' },
          el('p', { class: 'metric-note mono' }, identity(item)),
          !aligned(item)
            ? el(
                'p',
                { class: 'metric-note' },
                'This clock has no verified epoch placement. Inspect its original timestamps below.',
              )
            : item.points.length > 1
              ? plot(item)
              : el('p', {}, 'One observation; no trend can be established.'),
        ),
        el(
          'details',
          {},
          el(
            'summary',
            { class: 'panel-body' },
            `${numeric(item.returned_points ?? item.points.length)} of ${numeric(item.point_count ?? item.points.length)} observations · inspect values`,
          ),
          el(
            'div',
            { class: 'table-scroll resource-values' },
            el(
              'table',
              {},
              el(
                'thead',
                {},
                el(
                  'tr',
                  {},
                  el(
                    'th',
                    {},
                    aligned(item)
                      ? 'Offset from run start'
                      : 'Original timestamp · µs',
                  ),
                  el('th', {}, 'Process'),
                  el('th', { class: 'numeric' }, item.unit),
                ),
              ),
              el(
                'tbody',
                {},
                ...item.points
                  .slice(0, 2_000)
                  .map((point) =>
                    el(
                      'tr',
                      {},
                      el(
                        'td',
                        {},
                        aligned(item)
                          ? duration(
                              (point.timestamp_us - context.run.started_at_us) *
                                1_000,
                            )
                          : numeric(point.timestamp_us),
                      ),
                      el('td', { class: 'mono' }, item.pid),
                      el('td', { class: 'numeric' }, numeric(point.value)),
                    ),
                  ),
              ),
            ),
          ),
          item.truncated || item.points.length > 2_000
            ? el(
                'p',
                { class: 'table-foot' },
                'This page contains part of the series. Use the next resource page for more values.',
              )
            : null,
        ),
      ),
    ),
  );
  if (returned_points !== undefined && total_points !== undefined)
    body.append(
      el(
        'p',
        { class: 'metric-note' },
        `${numeric(returned_points)} of ${numeric(total_points)} resource points on this page`,
      ),
    );
  async function showPage(page?: string): Promise<void> {
    if (context.signal.aborted) return;
    body.replaceChildren(loading('Loading more resource series…'));
    try {
      const next = await resources(context, page);
      if (!context.signal.aborted) body.replaceWith(next);
    } catch (error) {
      if (context.signal.aborted) return;
      body.replaceChildren(
        errorView(error, () => {
          void showPage(page);
        }),
      );
    }
  }
  if (next_cursor)
    body.append(
      button('Next resource page', () => {
        void showPage(next_cursor);
      }),
    );
  if (cursor)
    body.append(
      button(
        'First resource page',
        () => {
          void showPage();
        },
        'button subtle',
      ),
    );
  return body;
}
