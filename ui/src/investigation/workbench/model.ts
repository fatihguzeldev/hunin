import type { OperationOutputs } from '../../operations.generated';
import type { TimeWindow } from './selection';

export type Inspection = OperationOutputs['profiles.inspect'];
export type ProfileFunction = Inspection['functions'][number];
export type SelectedFunction = NonNullable<Inspection['selected']>;
export type ProfileNode = Inspection['roots'][number];
export type ActivityBin = Inspection['activity'][number];

export function microtime(value: number): string {
  const magnitude = Math.abs(value);
  return magnitude < 1_000
    ? `${Math.round(value)} µs`
    : magnitude < 1_000_000
      ? `${new Intl.NumberFormat('en', { maximumFractionDigits: 1 }).format(value / 1_000)} ms`
      : `${new Intl.NumberFormat('en', { maximumFractionDigits: 2 }).format(value / 1_000_000)} s`;
}

export function timeLabel(window: TimeWindow, origin: number): string {
  return `${microtime(window.start_us - origin)}–${microtime(window.end_us - origin)}`;
}

export function rows(
  functions: readonly ProfileFunction[],
  query: string,
  sort: 'self' | 'inclusive',
): ProfileFunction[] {
  const search = query.trim().toLocaleLowerCase();
  const key = sort === 'self' ? 'self_weight' : 'inclusive_weight';
  return functions
    .filter((frame) =>
      `${frame.function} ${frame.file ?? ''} ${frame.module ?? ''}`
        .toLocaleLowerCase()
        .includes(search),
    )
    .sort(
      (a, b) =>
        (b[key] ?? -Infinity) - (a[key] ?? -Infinity) ||
        a.function.localeCompare(b.function) ||
        a.id.localeCompare(b.id),
    );
}

export function nodeAt(
  roots: ProfileNode[],
  path: readonly string[],
): ProfileNode | undefined {
  let current: ProfileNode | undefined;
  let children = roots;
  for (const id of path) {
    current = children.find((node) => node.id === id);
    if (!current) return undefined;
    children = current.children;
  }
  return current;
}

export function framePath(
  roots: ProfileNode[],
  frameId: string,
): string[] | undefined {
  let best: { path: string[]; weight: number } | undefined;
  function visit(nodes: ProfileNode[], path: string[]) {
    for (const node of nodes) {
      const next = [...path, node.id];
      const measured = node.total_weight ?? -Infinity;
      if (node.frame_id === frameId && (!best || measured > best.weight))
        best = { path: next, weight: measured };
      visit(node.children, next);
    }
  }
  visit(roots, []);
  return best?.path;
}

export function shortPath(path: string | null | undefined): string {
  return (
    path
      ?.replaceAll('\\', '/')
      .split('/')
      .filter(Boolean)
      .slice(-2)
      .join('/') || 'Source unavailable'
  );
}
