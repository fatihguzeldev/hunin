import { numeric } from '../format.js';
import { weight } from '../profile-weight.js';
import type { CodeCoverageBucket } from '../operations.generated.js';

/** Sample count and sample weight coincide only for unweighted sample observations. */
export function coverageText(bucket: CodeCoverageBucket, unit: string): string {
  const samples = `${numeric(bucket.sample_count)} samples`;
  return unit === 'samples' && bucket.weight === bucket.sample_count
    ? samples
    : `${samples} · ${weight(bucket.weight, unit)}`;
}
