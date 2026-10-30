import type { PageLocation } from '../pagination.js';

export type ProfileInvestigation = {
  unit: string;
  path: string[];
  cursor?: string;
  breadcrumbs: string[];
  mode: 'tree' | 'flame';
  query: string;
  scopeKey?: string;
  expanded?: string[];
  focus?: { path: string[]; names: string[] };
};

export type TimelineInvestigation = {
  kind: '' | 'sample' | 'span' | 'event' | 'metric' | 'context' | 'relation';
  pid?: number;
  alignment: 'aligned' | 'unmapped';
  window?: { start_us: number; end_us: number };
  cursor?: string;
  selectedId?: string;
};

export type EvidenceSelection =
  | { kind: 'observation'; id: string }
  | { kind: 'artifact'; id: string; name: string; offset: number };

export type EvidenceInvestigation = {
  cursor?: string;
  artifacts?: PageLocation;
  selection?: EvidenceSelection;
};

export type SourceSelection = {
  view: string;
  path: string;
  line?: number;
  codeId?: string;
  startLine?: number;
};

export type Investigation = {
  profile: ProfileInvestigation;
  timeline: TimelineInvestigation;
  evidence: EvidenceInvestigation;
  comparison: {
    before?: string;
    after?: string;
    unit?: string;
    metric?: string;
  };
  resources: PageLocation;
  source?: SourceSelection;
};

type SavedInvestigation = {
  version: 1;
  resultIdentity?: string;
  state: Investigation;
};

type Entry = SavedInvestigation & { restored: boolean };
type SessionStorage = Pick<Storage, 'getItem' | 'setItem'> &
  Partial<Pick<Storage, 'removeItem'>>;

function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function strings(value: unknown, limit = 128): string[] | undefined {
  return Array.isArray(value) &&
    value.length <= limit &&
    value.every((item): item is string => typeof item === 'string')
    ? [...value]
    : undefined;
}

function pageLocation(value: unknown): PageLocation | undefined {
  const location = object(value);
  if (
    !location ||
    !Array.isArray(location.previous) ||
    !location.previous.every(
      (cursor): cursor is string | null =>
        cursor === null || typeof cursor === 'string',
    )
  )
    return undefined;
  return { cursor: text(location.cursor), previous: [...location.previous] };
}

function empty(): Investigation {
  return {
    profile: { unit: '', path: [], breadcrumbs: [], mode: 'tree', query: '' },
    timeline: { kind: '', alignment: 'aligned' },
    evidence: {},
    comparison: {},
    resources: { previous: [] },
  };
}

function decode(value: unknown): SavedInvestigation | undefined {
  const saved = object(value);
  const state = object(saved?.state);
  const profile = object(state?.profile);
  if (saved?.version !== 1 || !profile) return undefined;
  const result = empty();
  result.profile.unit = text(profile.unit) ?? '';
  result.profile.path = strings(profile.path) ?? [];
  result.profile.breadcrumbs = strings(profile.breadcrumbs) ?? [];
  result.profile.cursor = text(profile.cursor);
  result.profile.mode = profile.mode === 'flame' ? 'flame' : 'tree';
  result.profile.query = text(profile.query) ?? '';
  result.profile.scopeKey = text(profile.scopeKey);
  result.profile.expanded = strings(profile.expanded, 2_000);
  const focus = object(profile.focus);
  const focusPath = strings(focus?.path, 256);
  const focusNames = strings(focus?.names, 256);
  if (focusPath && focusNames && focusPath.length === focusNames.length)
    result.profile.focus = { path: focusPath, names: focusNames };
  const timeline = object(state?.timeline);
  if (timeline) {
    const kind = timeline.kind;
    if (
      kind === '' ||
      kind === 'sample' ||
      kind === 'span' ||
      kind === 'event' ||
      kind === 'metric' ||
      kind === 'context' ||
      kind === 'relation'
    )
      result.timeline.kind = kind;
    if (
      typeof timeline.pid === 'number' &&
      Number.isSafeInteger(timeline.pid) &&
      timeline.pid >= 0
    )
      result.timeline.pid = timeline.pid;
    result.timeline.alignment =
      timeline.alignment === 'unmapped' ? 'unmapped' : 'aligned';
    const range = object(timeline.window);
    if (
      range &&
      typeof range.start_us === 'number' &&
      typeof range.end_us === 'number' &&
      Number.isSafeInteger(range.start_us) &&
      Number.isSafeInteger(range.end_us) &&
      range.start_us >= 0 &&
      range.end_us >= range.start_us
    )
      result.timeline.window = {
        start_us: range.start_us,
        end_us: range.end_us,
      };
    result.timeline.cursor = text(timeline.cursor);
    result.timeline.selectedId = text(timeline.selectedId);
  }
  const evidence = object(state?.evidence);
  result.evidence.cursor = text(evidence?.cursor);
  const artifactPage = pageLocation(evidence?.artifacts);
  if (artifactPage) result.evidence.artifacts = artifactPage;
  result.resources = pageLocation(state?.resources) ?? { previous: [] };
  const selection = object(evidence?.selection);
  if (selection && typeof selection.id === 'string') {
    if (selection.kind === 'observation')
      result.evidence.selection = { kind: 'observation', id: selection.id };
    else if (
      selection.kind === 'artifact' &&
      typeof selection.name === 'string' &&
      typeof selection.offset === 'number' &&
      Number.isSafeInteger(selection.offset) &&
      selection.offset >= 0
    )
      result.evidence.selection = {
        kind: 'artifact',
        id: selection.id,
        name: selection.name,
        offset: selection.offset,
      };
  }
  const comparison = object(state?.comparison);
  result.comparison.before = text(comparison?.before) || undefined;
  result.comparison.after = text(comparison?.after) || undefined;
  const comparisonUnit = text(comparison?.unit);
  const comparisonMetric = text(comparison?.metric);
  if (comparisonUnit) result.comparison.unit = comparisonUnit;
  if (comparisonMetric) result.comparison.metric = comparisonMetric;
  const source = object(state?.source);
  const path = text(source?.path);
  const view = text(source?.view);
  if (
    source &&
    path &&
    view &&
    (source?.line === undefined ||
      (typeof source.line === 'number' &&
        Number.isInteger(source.line) &&
        source.line >= 1)) &&
    (source.codeId === undefined || typeof source.codeId === 'string') &&
    (source.startLine === undefined ||
      (typeof source.startLine === 'number' &&
        Number.isSafeInteger(source.startLine) &&
        source.startLine >= 1))
  ) {
    result.source = {
      view,
      path,
      line: source.line,
      codeId: text(source.codeId),
      startLine:
        typeof source.startLine === 'number' ? source.startLine : undefined,
    };
  }
  return {
    version: 1,
    resultIdentity: text(saved.resultIdentity),
    state: result,
  };
}

/** Own private investigation preferences; raw observations and source bytes stay in the service. */
export class Investigations {
  private entries = new Map<string, Entry>();
  persistent: boolean;

  constructor(private storage?: SessionStorage) {
    this.persistent = storage !== undefined;
  }

  private entry(runId: string): Entry {
    const existing = this.entries.get(runId);
    if (existing) return existing;
    let saved: SavedInvestigation | undefined;
    let value: string | null | undefined;
    try {
      value = this.storage?.getItem(`hunin.investigation.v1:${runId}`);
    } catch {
      this.persistent = false;
    }
    if (value) {
      try {
        saved = decode(JSON.parse(value));
      } catch {
        /* Malformed private preferences do not make storage unavailable. */
      }
    }
    const entry: Entry = saved
      ? { ...saved, restored: true }
      : { version: 1, state: empty(), restored: false };
    this.entries.set(runId, entry);
    return entry;
  }

  forRun(runId: string): Investigation {
    return this.entry(runId).state;
  }

  capture(runId: string): SavedInvestigation {
    const entry = this.entry(runId);
    return {
      version: 1,
      resultIdentity: entry.resultIdentity,
      state: entry.state,
    };
  }

  restore(runId: string, value: unknown): void {
    const saved = decode(value);
    if (saved) this.entries.set(runId, { ...saved, restored: true });
  }

  persist(runId: string): void {
    try {
      this.storage?.setItem(
        `hunin.investigation.v1:${runId}`,
        JSON.stringify(this.capture(runId)),
      );
    } catch {
      this.persistent = false;
    }
  }

  forget(runId: string): void {
    this.entries.set(runId, { version: 1, state: empty(), restored: false });
    const key = `hunin.investigation.v1:${runId}`;
    try {
      if (this.storage?.removeItem) this.storage.removeItem(key);
      else this.storage?.setItem(key, JSON.stringify(this.capture(runId)));
    } catch {
      this.persistent = false;
    }
  }

  observeResult(
    runId: string,
    identity: string | undefined,
  ): 'changed' | 'unconfirmed' | undefined {
    const entry = this.entry(runId);
    const changed =
      entry.resultIdentity !== undefined && identity !== entry.resultIdentity;
    const unconfirmed =
      entry.restored &&
      (identity === undefined || entry.resultIdentity === undefined);
    if (changed || unconfirmed) {
      const profile = entry.state.profile;
      profile.path = [];
      profile.breadcrumbs = [];
      profile.cursor = undefined;
      profile.scopeKey = undefined;
      profile.expanded = undefined;
      profile.focus = undefined;
      entry.state.source = undefined;
      entry.state.timeline.window = undefined;
      entry.state.timeline.cursor = undefined;
      entry.state.timeline.selectedId = undefined;
      entry.state.evidence.cursor = undefined;
      entry.state.evidence.artifacts = undefined;
      entry.state.resources = { previous: [] };
      if (entry.state.evidence.selection?.kind === 'observation')
        entry.state.evidence.selection = undefined;
    }
    entry.resultIdentity = identity;
    entry.restored = false;
    this.persist(runId);
    return changed ? 'changed' : unconfirmed ? 'unconfirmed' : undefined;
  }
}
