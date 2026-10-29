import js from '@eslint/js';
import globals from 'globals';
import tseslint from 'typescript-eslint';
import { defineConfig, globalIgnores } from 'eslint/config';

// Build systems can emit files such as compiler_depend.ts which are not
// TypeScript. Match component source, rather than arbitrary files on disk.
const ownedSources = [
  '*.mts',
  'quality/**/*.mts',
  'ui/src/**/*.ts',
  'ui/scripts/**/*.mts',
  'ui/test/**/*.mts',
  'ui/vite.config.ts',
  'collectors/javascript/src/**/*.{ts,mts}',
  'sdk/javascript/src/**/*.{ts,mts}',
  'sdk/conformance/*.mts',
];

export default defineConfig(
  globalIgnores([
    '**/node_modules/**',
    'target/**',
    'dist/**',
    'ui/dist/**',
    'ui/.tools/**',
    'context/**',
    '.hunin/**',
    // Recorded source maps and profiles refer to these exact source bytes/lines.
    'src/importers/fixtures/**',
    'tests/workloads/**',
    // These are emitted from the protocol authority, not independently edited.
    'ui/src/*.generated.ts',
    'sdk/javascript/*.d.mts',
    'sdk/javascript/test/**',
  ]),
  {
    files: ownedSources,
    extends: [js.configs.recommended],
    languageOptions: { ecmaVersion: 2022 },
    linterOptions: { reportUnusedDisableDirectives: 'error' },
  },
  {
    files: ownedSources,
    extends: [tseslint.configs.recommended],
  },
  {
    files: ['ui/src/**/*.ts'],
    languageOptions: { globals: globals.browser },
  },
  {
    files: [
      '*.mts',
      'quality/**/*.mts',
      'ui/scripts/**/*.mts',
      'ui/test/**/*.mts',
      'ui/vite.config.ts',
      'collectors/javascript/src/**/*.mts',
      'sdk/javascript/src/**/*.mts',
      'sdk/conformance/*.mts',
    ],
    languageOptions: { globals: globals.node },
  },
  {
    files: [
      'collectors/javascript/src/clock-preload.mts',
      'collectors/javascript/src/deno-workers.mts',
    ],
    languageOptions: { globals: { Deno: 'readonly' } },
  },
);
