import { button, el } from '../../dom';
import { icon } from '../../design/icons';
import { numeric } from '../../format';
import { weight } from '../../profile-weight';
import {
  microtime,
  timeLabel,
  type ActivityBin,
  type Inspection,
} from './model';
import {
  clampWindow,
  dragMode,
  panWindow,
  resizeWindow,
  zoomWindow,
  type TimeWindow,
} from './selection';
import { binAt, binGeometry } from './geometry';

const namespace = 'http://www.w3.org/2000/svg';
function svg<K extends keyof SVGElementTagNameMap>(
  name: K,
  attributes: Record<string, string | number>,
) {
  const node = document.createElementNS(namespace, name);
  for (const [key, value] of Object.entries(attributes))
    node.setAttribute(key, String(value));
  return node;
}

let chartIdentity = 0;
function plot(
  bins: ActivityBin[],
  unit: string,
  overview: boolean,
  domain: TimeWindow,
): SVGSVGElement {
  const width = 1200;
  const height = overview ? 34 : 124;
  const chart = svg('svg', {
    viewBox: `0 0 ${width} ${height}`,
    preserveAspectRatio: 'none',
    role: 'img',
    'aria-label': `Sampled activity, ${unit} per bin`,
  });
  const patternId = `wb-unknown-${++chartIdentity}`;
  const definitions = svg('defs', {});
  const pattern = svg('pattern', {
    id: patternId,
    width: 6,
    height: 6,
    patternUnits: 'userSpaceOnUse',
  });
  pattern.append(
    svg('path', { d: 'M-1 1l2-2M0 6l6-6M5 7l2-2', class: 'wb-unknown-stroke' }),
  );
  definitions.append(pattern);
  chart.append(definitions);
  const max = Math.max(1, ...bins.map((bin) => bin.weight ?? 0));
  for (const bin of bins) {
    const geometry = binGeometry(bin, domain);
    const x = (geometry.left / 100) * width;
    const barWidth = Math.max(
      0.2,
      (geometry.width / 100) * width - (overview ? 0.7 : 1.7),
    );
    const value = bin.weight;
    if (value === null) {
      if (geometry.unavailable) {
        const unknown = svg('rect', {
          x,
          y: 0,
          width: barWidth,
          height,
          fill: `url(#${patternId})`,
          'aria-label': `Unknown weight · ${numeric(bin.sample_count)} samples`,
        });
        const title = svg('title', {});
        title.textContent = `Unknown weight · ${numeric(bin.sample_count)} samples`;
        unknown.append(title);
        chart.append(unknown);
      }
      continue;
    }
    const barHeight = (value / max) * (height - 3);
    const bar = svg('rect', {
      x,
      y: height - barHeight,
      width: barWidth,
      height: barHeight,
      class: 'wb-activity-total',
    });
    const title = svg('title', {});
    title.textContent = `${weight(value, unit)} · ${numeric(bin.sample_count)} samples`;
    bar.append(title);
    chart.append(bar);
    if (bin.selected_weight !== null && bin.selected_weight > 0) {
      const selectedHeight =
        (Math.min(value, bin.selected_weight) / max) * (height - 3);
      chart.append(
        svg('rect', {
          x,
          y: height - selectedHeight,
          width: barWidth,
          height: selectedHeight,
          class: 'wb-activity-selected',
        }),
      );
    }
  }
  return chart;
}

export function activity(
  data: Inspection,
  select: (window?: TimeWindow) => void,
  signal: AbortSignal,
): HTMLElement {
  const bounds = data.bounds;
  const selected = data.window;
  const region = el('section', {
    class: 'wb-time',
    'aria-label': 'Sampled activity',
  });
  if (!bounds || !selected) return region;
  const toolbar = el(
    'div',
    { class: 'wb-time-toolbar' },
    el('h2', {}, 'Timeline'),
  );
  const controls = el('div', { class: 'wb-time-actions' });
  for (const [name, drawing, action] of [
    [
      'Previous window',
      'previous',
      () =>
        select(
          panWindow(
            selected,
            bounds,
            -(selected.end_us - selected.start_us) * 0.8,
          ),
        ),
    ],
    ['Zoom out', 'minus', () => select(zoomWindow(selected, bounds, 2))],
    ['Zoom in', 'plus', () => select(zoomWindow(selected, bounds, 0.5))],
    ['Entire recording', 'fit', () => select(undefined)],
    [
      'Next window',
      'next',
      () =>
        select(
          panWindow(
            selected,
            bounds,
            (selected.end_us - selected.start_us) * 0.8,
          ),
        ),
    ],
  ] as const) {
    const control = button('', action, 'button subtle wb-icon-button');
    control.append(icon(drawing));
    control.setAttribute('aria-label', name);
    control.title = name;
    controls.append(control);
  }
  toolbar.append(controls);
  const overview = el('div', {
    class: 'wb-overview',
    tabindex: 0,
    role: 'group',
    'aria-label':
      'Time window. Drag to select; drag the selection to move it. Arrow keys pan, plus and minus zoom, Home shows the recording.',
  });
  overview.append(plot(data.overview, data.weight_unit, true, bounds));
  const selection = el('div', { class: 'wb-time-selection' });
  const startHandle = el('span', { class: 'wb-time-handle start' });
  const endHandle = el('span', { class: 'wb-time-handle end' });
  selection.append(startHandle, endHandle);
  overview.append(selection);
  const preview = el(
    'span',
    { class: 'wb-window-label mono', 'aria-live': 'polite' },
    timeLabel(selected, bounds.start_us),
  );
  const display = (window: TimeWindow) => {
    const span = Math.max(1, bounds.end_us - bounds.start_us);
    selection.style.left = `${((window.start_us - bounds.start_us) / span) * 100}%`;
    selection.style.width = `${Math.max(0.2, ((window.end_us - window.start_us) / span) * 100)}%`;
    preview.textContent = timeLabel(window, bounds.start_us);
  };
  display(selected);
  let drag:
    | {
        id: number;
        anchor: number;
        original: TimeWindow;
        next: TimeWindow;
        mode: 'new' | 'move' | 'start' | 'end';
      }
    | undefined;
  const timestamp = (event: PointerEvent) => {
    const rect = overview.getBoundingClientRect();
    return Math.round(
      bounds.start_us +
        Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)) *
          (bounds.end_us - bounds.start_us),
    );
  };
  overview.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    overview.focus({ preventScroll: true });
    const target = event.target;
    const hit =
      target === startHandle
        ? 'start'
        : target === endHandle
          ? 'end'
          : target instanceof Node && selection.contains(target)
            ? 'move'
            : 'new';
    const mode = dragMode(hit, selected, bounds);
    drag = {
      id: event.pointerId,
      anchor: timestamp(event),
      original: { ...selected },
      next: { ...selected },
      mode,
    };
    overview.setPointerCapture(event.pointerId);
  });
  overview.addEventListener('pointermove', (event) => {
    if (!drag || drag.id !== event.pointerId) return;
    const next = timestamp(event);
    drag.next =
      drag.mode === 'move'
        ? panWindow(drag.original, bounds, next - drag.anchor)
        : drag.mode === 'new'
          ? clampWindow(
              {
                start_us: Math.min(next, drag.anchor),
                end_us: Math.max(next, drag.anchor),
              },
              bounds,
            )
          : resizeWindow(drag.original, bounds, drag.mode, next);
    display(drag.next);
  });
  overview.addEventListener('pointerup', (event) => {
    if (!drag || drag.id !== event.pointerId) return;
    const current = drag;
    drag = undefined;
    overview.releasePointerCapture(event.pointerId);
    if (current.mode === 'new' && current.anchor === timestamp(event)) {
      const width = selected.end_us - selected.start_us;
      select(
        clampWindow(
          {
            start_us: current.anchor - width / 2,
            end_us: current.anchor + width / 2,
          },
          bounds,
        ),
      );
    } else select(current.next);
  });
  overview.addEventListener('pointercancel', () => {
    drag = undefined;
    display(selected);
  });
  overview.addEventListener('keydown', (event) => {
    const width = selected.end_us - selected.start_us;
    const action =
      event.key === 'ArrowLeft'
        ? () => select(panWindow(selected, bounds, -width * 0.2))
        : event.key === 'ArrowRight'
          ? () => select(panWindow(selected, bounds, width * 0.2))
          : ['+', '=', 'ArrowUp'].includes(event.key)
            ? () => select(zoomWindow(selected, bounds, 0.5))
            : ['-', 'ArrowDown'].includes(event.key)
              ? () => select(zoomWindow(selected, bounds, 2))
              : event.key === 'Home'
                ? () => select(undefined)
                : undefined;
    if (action) {
      event.preventDefault();
      action();
    }
  });
  const chart = el(
    'div',
    {
      class: 'wb-detail-plot',
      tabindex: 0,
      role: 'group',
      'aria-label':
        'Selected-window activity. Control or Command plus scroll zooms around the pointer.',
    },
    plot(data.activity, data.weight_unit, false, selected),
  );
  let wheelWindow = selected;
  let wheelTimer: ReturnType<typeof setTimeout> | undefined;
  signal.addEventListener('abort', () => clearTimeout(wheelTimer), {
    once: true,
  });
  chart.addEventListener(
    'wheel',
    (event) => {
      if (!event.ctrlKey && !event.metaKey) return;
      event.preventDefault();
      const rect = chart.getBoundingClientRect();
      wheelWindow = zoomWindow(
        wheelWindow,
        bounds,
        event.deltaY > 0 ? 1.35 : 1 / 1.35,
        (event.clientX - rect.left) / rect.width,
      );
      clearTimeout(wheelTimer);
      wheelTimer = setTimeout(() => {
        if (!signal.aborted) select(wheelWindow);
      }, 140);
    },
    { passive: false },
  );
  const binDetail = el('output', {
    class: 'wb-bin-detail mono',
    'aria-live': 'polite',
  });
  let inspected = -1;
  const inspect = (index: number) => {
    inspected = Math.max(0, Math.min(data.activity.length - 1, index));
    const bin = data.activity[inspected];
    if (!bin) return;
    binDetail.textContent = `${timeLabel(bin, bounds.start_us)} · ${weight(bin.weight, data.weight_unit)} · ${numeric(bin.sample_count)} samples`;
    const geometry = binGeometry(bin, selected);
    chart.style.setProperty(
      '--wb-inspected-bin',
      `${geometry.left + geometry.width / 2}%`,
    );
    chart.classList.add('inspecting');
  };
  const clearInspection = () => {
    chart.classList.remove('inspecting');
    binDetail.textContent = '';
    inspected = -1;
  };
  chart.addEventListener('pointermove', (event) => {
    const rect = chart.getBoundingClientRect();
    const timestamp =
      selected.start_us +
      Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)) *
        (selected.end_us - selected.start_us);
    const index = binAt(data.activity, timestamp);
    if (index !== inspected) inspect(index);
  });
  chart.addEventListener('pointerleave', clearInspection);
  chart.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      clearInspection();
    } else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
      event.preventDefault();
      inspect(
        inspected < 0 ? 0 : inspected + (event.key === 'ArrowLeft' ? -1 : 1),
      );
    }
  });
  const ticks = el('div', { class: 'wb-time-ticks mono' });
  for (let index = 0; index < 5; index++)
    ticks.append(
      el(
        'span',
        {},
        microtime(
          selected.start_us -
            bounds.start_us +
            ((selected.end_us - selected.start_us) * index) / 4,
        ),
      ),
    );
  region.append(
    toolbar,
    overview,
    el(
      'div',
      { class: 'wb-window-summary' },
      preview,
      el(
        'span',
        {},
        `${numeric(data.sample_count)} of ${numeric(data.total_sample_count)} samples`,
      ),
    ),
    el(
      'div',
      { class: 'wb-chart-legend' },
      el(
        'span',
        { class: 'wb-total-legend' },
        `Sampled weight · ${data.weight_unit}/bin`,
      ),
      data.selected
        ? el(
            'span',
            { class: 'wb-selected-legend' },
            `${data.selected.function} · self`,
          )
        : null,
      data.activity.some((bin) => bin.weight === null && bin.sample_count > 0)
        ? el(
            'span',
            { class: 'wb-unknown-legend' },
            'Patterned bins: weight unavailable',
          )
        : null,
    ),
    chart,
    ticks,
    binDetail,
  );
  return region;
}
