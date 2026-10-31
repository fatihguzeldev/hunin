import { totalWeight } from '../../profile-weight.js';
import type { TimeWindow } from './selection';

/** Position bins on their returned time edges, not an assumed equal-width index. */
export function binGeometry(
  bin: TimeWindow & { weight: number | null; sample_count: number },
  domain: TimeWindow,
) {
  const span = Math.max(1, domain.end_us - domain.start_us);
  const start = Math.max(
    domain.start_us,
    Math.min(domain.end_us, bin.start_us),
  );
  const end = Math.max(start, Math.min(domain.end_us, bin.end_us));
  return {
    left: ((start - domain.start_us) / span) * 100,
    width: ((end - start) / span) * 100,
    unavailable: bin.weight === null && bin.sample_count > 0,
  };
}

export function binAt(
  bins: readonly { start_us: number }[],
  timestamp: number,
): number {
  let low = 0;
  let high = bins.length;
  while (low < high) {
    const middle = Math.floor((low + high) / 2);
    if (bins[middle].start_us <= timestamp) low = middle + 1;
    else high = middle;
  }
  return Math.max(0, Math.min(bins.length - 1, low - 1));
}

export function stackScope(
  complete: number | null,
  shown: readonly (number | null)[],
  focused: boolean,
) {
  const visible = totalWeight(shown);
  return {
    denominator: focused ? visible : complete,
    omitted: focused
      ? 0
      : complete === null || visible === null
        ? null
        : Math.max(0, complete - visible),
    focused,
  };
}
