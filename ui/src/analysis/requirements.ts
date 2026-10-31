import type { OperationInputs } from '../operations.generated.js';

type SourceIdentity = {
  fingerprint: string;
  executable_sha256?: string;
  runtime_version?: string;
};

function known(value?: string): string | undefined {
  return value && value !== 'unknown' ? value : undefined;
}

export function analyzerRequirements(
  scope: 'recording' | 'independent',
  runtime: string,
  source: SourceIdentity,
): OperationInputs['analyses.register']['requirements'] {
  if (!runtime.trim())
    throw new Error('Choose a required runtime, or * for any runtime.');
  if (scope === 'independent')
    return { runtime: runtime.trim(), source_independent: true };
  const fingerprint = known(source.fingerprint);
  if (!fingerprint)
    throw new Error(
      'This recording has no source fingerprint. Choose source-independent only if the analyzer does not depend on source.',
    );
  return {
    runtime: runtime.trim(),
    source_fingerprint: fingerprint,
    ...(known(source.executable_sha256)
      ? { executable_sha256: source.executable_sha256 }
      : {}),
    ...(known(source.runtime_version)
      ? { runtime_version: source.runtime_version }
      : {}),
  };
}
