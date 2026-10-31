import type { OperationOutputs } from '../operations.generated.js';

/** Display source availability without treating byte identity as a build mapping. */
export function sourcePresentation(data: OperationOutputs['source.read']) {
  const origin =
    data.source_kind === 'current_file'
      ? 'Current file'
      : data.source_kind === 'recorded_snapshot'
        ? 'Saved source'
        : data.source_kind === 'recorded_source_map'
          ? 'Captured source'
          : 'Source';
  const content =
    data.content !== '' ||
    (data.total_lines !== undefined &&
      data.total_lines > 0 &&
      data.start_line <= data.total_lines)
      ? 'lines'
      : data.total_lines === 0
        ? 'empty-file'
        : data.total_lines !== undefined && data.start_line > data.total_lines
          ? 'outside-window'
          : 'unknown-window';
  const verifiedBytes =
    data.source_kind === 'current_file'
      ? 'Current bytes are verified.'
      : data.source_kind === 'recorded_snapshot'
        ? 'Saved bytes are verified.'
        : data.source_kind === 'recorded_source_map'
          ? 'Captured bytes are verified.'
          : 'Source bytes are verified.';
  return {
    origin,
    content,
    identity: data.identity_verified ? 'Bytes verified' : 'Bytes unverified',
    mapping:
      data.build_mapping_verified === true
        ? 'Build mapping verified'
        : 'Build mapping unverified',
    tone: data.identity_verified ? '' : 'warn',
    note: data.identity_verified
      ? data.build_mapping_verified === true
        ? `${verifiedBytes} The source-to-build mapping is verified.`
        : `${verifiedBytes} The source-to-build mapping is unverified.`
      : data.source_kind === 'current_file'
        ? 'This file may have changed since the recording.'
        : 'These source bytes are not verified against the recording.',
  } as const;
}
