import type { Observation } from './record.generated';

function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

export function isModuleLifetime(record: Observation): boolean {
  return (
    record.name === 'module.observed_lifetime' &&
    record.attributes?.scope === 'module'
  );
}

export function observationLabel(record: Observation): string {
  if (isModuleLifetime(record)) {
    const attributes = record.attributes!;
    const module =
      typeof attributes.module === 'string'
        ? attributes.module.replaceAll('\\', '/').split('/').at(-1)
        : 'Unknown module';
    return `${module} · ${attributes.process_instance_id ?? record.pid}/${attributes.mapping_instance_id ?? 'unknown load'}`;
  }
  return record.name ?? record.stack?.at(-1)?.function ?? record.kind;
}

function nativeLoads(record: Observation): Record<string, unknown>[] {
  const value = record.attributes?.native_load_instances;
  return Array.isArray(value)
    ? value
        .map(object)
        .filter((item): item is Record<string, unknown> => Boolean(item))
    : [];
}

export function referenceIds(record: Observation): string[] {
  return [
    ...new Set(
      [
        record.context_id,
        record.parent_id,
        record.from,
        record.to,
        ...(record.evidence ?? []),
        ...nativeLoads(record).map((value) => value.record_id),
      ].filter((id): id is string => typeof id === 'string' && id.length > 0),
    ),
  ];
}

/** Display recorded boundary and identity fields without deriving a lifetime. */
export function nativeEvidenceRows(record: Observation): [string, string][] {
  const attributes = record.attributes ?? {};
  const rows: [string, string][] = [];
  const value = (name: string, field: unknown): void => {
    if (field !== undefined)
      rows.push([
        name,
        typeof field === 'string' ? field : JSON.stringify(field),
      ]);
  };
  value('Native mapping status', attributes.native_mapping_status);
  if (
    !isModuleLifetime(record) &&
    attributes.source_format === 'samply-native-lifetimes'
  ) {
    value(
      'Mapping observation time · ms from profile epoch',
      attributes.mapping_time_ms,
    );
    value('Evidence complete', attributes.evidence_complete);
  }
  if (isModuleLifetime(record)) {
    value('Module', attributes.module);
    value('Load instance', attributes.mapping_instance_id);
    value('Process instance', attributes.process_instance_id);
    value('Address start', attributes.start_avma);
    value('Address end', attributes.end_avma);
    value('Start window · ms from profile epoch', attributes.start_window_ms);
    value('End window · ms from profile epoch', attributes.end_window_ms);
    value('Start censored', attributes.start_censored);
    value('End censored', attributes.end_censored);
    value('Boundary basis', attributes.boundary_basis);
    value('Exact event timestamps', attributes.exact_event_timestamps);
    value('Evidence complete', attributes.evidence_complete);
    value('End reason', attributes.end_reason);
    value('Recorded library identity', attributes.library);
  }
  nativeLoads(record).forEach((mapping, index) =>
    value(`Native frame mapping ${index + 1}`, mapping),
  );
  return rows;
}
