import js from '@eslint/js';
import globals from 'globals';
import tseslint from 'typescript-eslint';
import { defineConfig, globalIgnores } from 'eslint/config';

export default defineConfig(
  globalIgnores([
    '**/node_modules/**',
    'target/**',
    'dist/**',
    'ui/dist/**',
    'context/**',
    '.hunin/**',
    // Recorded source maps and profiles refer to these exact source bytes/lines.
    'src/importers/fixtures/**',
    'tests/workloads/**',
    // These are emitted from the protocol authority, not independently edited.
    'ui/src/*.generated.ts',
  ]),
  {
    files: ['**/*.{js,mjs,cjs,ts}'],
    extends: [js.configs.recommended],
    languageOptions: { ecmaVersion: 2022 },
    linterOptions: { reportUnusedDisableDirectives: 'error' },
  },
  {
    files: ['**/*.ts'],
    extends: [tseslint.configs.recommended],
  },
  {
    files: ['ui/src/**/*.ts'],
    languageOptions: { globals: globals.browser },
  },
  {
    files: [
      '*.mjs',
      'quality/**/*.mjs',
      'ui/scripts/**/*.mjs',
      'ui/test/**/*.mjs',
      'ui/vite.config.ts',
      'collectors/javascript/**/*.mjs',
      'sdk/javascript/**/*.{mjs,ts}',
      'sdk/conformance/*.mjs',
    ],
    languageOptions: { globals: globals.node },
  },
  {
    files: [
      'collectors/javascript/clock-preload.mjs',
      'collectors/javascript/deno-workers.mjs',
    ],
    languageOptions: { globals: { Deno: 'readonly' } },
  },
);
