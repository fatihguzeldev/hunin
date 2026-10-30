import type { Run, Summary } from '../api';
import type { Investigation } from './state';

export type ViewContext = {
  run: Run;
  summary?: Summary;
  runs: readonly Run[];
  investigation: Investigation;
  signal: AbortSignal;
  persist: (mode?: 'push' | 'replace') => void;
  refresh: (message?: string) => void;
  followAttempt: (id: string) => void;
  source: (path: string, line?: number | null, codeId?: string | null) => void;
};

export type View = {
  name: string;
  render: (context: ViewContext) => Promise<HTMLElement>;
};
