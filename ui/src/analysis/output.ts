function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

export function outputArtifact(
  value: unknown,
): { id: string; name: string } | undefined {
  const artifacts = object(value)?.raw_artifacts;
  if (!Array.isArray(artifacts)) return undefined;
  for (const value of artifacts) {
    const artifact = object(value);
    if (
      artifact?.name === 'stdout.jsonl' &&
      artifact.format === 'analysis-output' &&
      typeof artifact.id === 'string' &&
      artifact.id
    )
      return { id: artifact.id, name: artifact.name };
  }
  return undefined;
}

/** Excerpt IDs are navigation hints; evidence.get supplies the canonical observation. */
export function outputSelections(
  content: string,
  runId: string,
  partialStart: boolean,
  complete: boolean,
): { id: string; label: string }[] {
  const lines = content.split('\n');
  if (partialStart) lines.shift();
  if (!complete) lines.pop();
  const selections = new Map<string, { id: string; label: string }>();
  for (const line of lines) {
    try {
      const record = object(JSON.parse(line));
      if (
        record?.run_id === runId &&
        record.basis === 'derived' &&
        typeof record.id === 'string' &&
        record.id &&
        typeof record.kind === 'string'
      )
        selections.set(record.id, {
          id: record.id,
          label:
            typeof record.name === 'string' && record.name
              ? record.name
              : record.kind,
        });
    } catch {
      // Partial lines and lossy raw excerpts never establish a published record.
    }
  }
  return [...selections.values()];
}
