import { duration, numeric } from './format.js';

export function weight(value: number | null, unit: string): string {
  if (value === null || !Number.isFinite(value))
    return `Unknown weight · ${unit}`;
  return unit === 'microseconds' && Number.isFinite(value * 1_000)
    ? duration(value * 1_000)
    : `${numeric(value)} ${unit}`;
}

export function weightShare(
  value: number | null,
  total: number | null,
): number | undefined {
  if (
    value === null ||
    total === null ||
    !Number.isFinite(value) ||
    !Number.isFinite(total) ||
    total <= 0
  )
    return undefined;
  return Math.max(0, Math.min(100, (value / total) * 100));
}

export function totalWeight(values: readonly (number | null)[]): number | null {
  let total = 0;
  for (const value of values) {
    if (value === null || !Number.isFinite(value)) return null;
    total += value;
  }
  return Number.isFinite(total) ? total : null;
}
