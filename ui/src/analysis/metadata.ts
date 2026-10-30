function document(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function strings(value: unknown): string[] | undefined {
  return Array.isArray(value) &&
    value.every((item): item is string => typeof item === 'string')
    ? value
    : undefined;
}

function number(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value)
    ? value
    : undefined;
}

function instant(value: unknown): string | undefined {
  const microseconds = number(value);
  if (microseconds === undefined) return undefined;
  const date = new Date(microseconds / 1_000);
  return Number.isNaN(date.valueOf()) ? undefined : date.toISOString();
}

/** Legacy documents remain raw evidence; missing display fields are not facts. */
export function analysisMetadata(value: unknown) {
  const row = document(value);
  return {
    id: text(row.id),
    name: text(row.name) ?? 'Unknown analyzer',
    version: text(row.version),
    command: strings(row.command),
    requirements: row.requirements,
    dependencies: row.dependencies,
    raw: value,
  };
}

export function analysisAttemptMetadata(value: unknown) {
  const row = document(value);
  return {
    id: text(row.id),
    result_set_id: text(row.result_set_id),
    analysis_id: text(row.analysis_id),
    version: text(row.version),
    status: text(row.status) ?? 'unknown',
    selected: typeof row.selected === 'boolean' ? row.selected : undefined,
    diagnostics: strings(row.diagnostics) ?? [],
    created_at: instant(row.created_at_us),
    input_fingerprint: text(row.input_fingerprint),
    raw: value,
  };
}
