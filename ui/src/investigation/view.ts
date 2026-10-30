import type { Run, Summary } from '../api';
import type { Investigation } from './state';
import type { FunctionSelection } from '../call-counts';
import type { ImportFormat } from '../operations.generated';

export type ViewContext = {
  run: Run;
  summary?: Summary;
  runs: readonly Run[];
  importFormats: (
    signal: AbortSignal,
  ) => Promise<readonly ImportFormat[] | undefined>;
  investigation: Investigation;
  signal: AbortSignal;
  persist: (mode?: 'push' | 'replace') => void;
  refresh: (message?: string) => void;
  followAttempt: (id: string) => void;
  countFunctions: (functions: FunctionSelection[]) => void;
  source: (path: string, line?: number | null, codeId?: string | null) => void;
};

export type View = {
  name: string;
  render: (context: ViewContext) => Promise<HTMLElement>;
};
