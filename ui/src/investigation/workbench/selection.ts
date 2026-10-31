export type TimeWindow = { start_us: number; end_us: number };

export function clampWindow(
  window: TimeWindow,
  bounds: TimeWindow,
): TimeWindow {
  const available = bounds.end_us - bounds.start_us;
  if (
    !Number.isFinite(window.start_us) ||
    !Number.isFinite(window.end_us) ||
    available <= 0
  )
    return { ...bounds };
  const width = Math.max(
    1,
    Math.min(available, Math.round(window.end_us - window.start_us)),
  );
  const start_us = Math.max(
    bounds.start_us,
    Math.min(bounds.end_us - width, Math.round(window.start_us)),
  );
  return { start_us, end_us: start_us + width };
}

export function zoomWindow(
  window: TimeWindow,
  bounds: TimeWindow,
  factor: number,
  anchor = 0.5,
): TimeWindow {
  const fraction = Math.max(0, Math.min(1, anchor));
  const width = window.end_us - window.start_us;
  const next = Math.max(
    1,
    Math.min(bounds.end_us - bounds.start_us, Math.round(width * factor)),
  );
  const fixed = window.start_us + width * fraction;
  return clampWindow(
    {
      start_us: fixed - next * fraction,
      end_us: fixed + next * (1 - fraction),
    },
    bounds,
  );
}

export function panWindow(
  window: TimeWindow,
  bounds: TimeWindow,
  delta: number,
): TimeWindow {
  return clampWindow(
    { start_us: window.start_us + delta, end_us: window.end_us + delta },
    bounds,
  );
}

export function resizeWindow(
  window: TimeWindow,
  bounds: TimeWindow,
  edge: 'start' | 'end',
  timestamp: number,
): TimeWindow {
  return edge === 'start'
    ? {
        start_us: Math.max(
          bounds.start_us,
          Math.min(window.end_us - 1, Math.round(timestamp)),
        ),
        end_us: window.end_us,
      }
    : {
        start_us: window.start_us,
        end_us: Math.min(
          bounds.end_us,
          Math.max(window.start_us + 1, Math.round(timestamp)),
        ),
      };
}

type Branch<T> = {
  frame_id: string;
  self_weight: number | null;
  total_weight: number | null;
  child_count: number;
  children_truncated: boolean;
  depth_truncated: boolean;
  children: T[];
};

/** Shorten only complete common ancestry; observed self weight remains visible. */
export function commonChain<T extends Branch<T>>(
  roots: T[],
  selected?: string,
) {
  const ancestors: T[] = [];
  let visible = roots;
  while (visible.length === 1) {
    const current = visible[0];
    if (
      current.frame_id === selected ||
      current.self_weight !== 0 ||
      current.total_weight === null ||
      current.child_count !== 1 ||
      current.children.length !== 1 ||
      current.children_truncated ||
      current.depth_truncated ||
      current.children[0].total_weight !== current.total_weight
    )
      break;
    ancestors.push(current);
    visible = current.children;
  }
  return { ancestors, roots: visible };
}
