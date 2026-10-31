import type { Observation } from '../record.generated.js';

type SelectedFrame = {
  function: string;
  file?: string | null;
  module?: string | null;
  code_id?: string | null;
  evidence_truncated?: boolean;
};

/** Anchors are retained sample occurrences, never inferred from a function name alone. */
export function mappingAnchors(
  records: readonly Observation[],
  selected: SelectedFrame,
  limit = 50,
) {
  const anchors: {
    observation_id: string;
    frame_index: number;
    timestamp_us: number;
    code_instance_id?: string;
    code_offset?: string;
  }[] = [];
  for (const record of records) {
    if (record.kind !== 'sample') continue;
    for (const [index, frame] of record.stack.entries()) {
      if (
        frame.function !== selected.function ||
        (frame.file ?? null) !== (selected.file ?? null) ||
        (frame.module ?? null) !== (selected.module ?? null) ||
        (frame.code_id ?? null) !== (selected.code_id ?? null)
      )
        continue;
      if (anchors.length === limit) return { anchors, truncated: true };
      anchors.push({
        observation_id: record.id,
        frame_index: index,
        timestamp_us: record.timestamp_us,
        code_instance_id: frame.code_instance_id,
        code_offset: frame.code_offset,
      });
    }
  }
  return { anchors, truncated: selected.evidence_truncated === true };
}
