import inspector from 'node:inspector';
import process from 'node:process';
import { createHash } from 'node:crypto';
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { threadId, isMainThread } from 'node:worker_threads';

const directory = process.env.HUNIN_JS_PROFILE_DIR;
const maximum = Number(process.env.HUNIN_CODE_MAX_BYTES ?? 67_108_864);
// Retain collector intent and actual startup arguments before target code runs.
// User-provided Node flags can override requested compiler scope.
const execArgv = [...process.execArgv];
const compilerExport = process.env.HUNIN_CODE_COMPILER_EXPORT === '1';
const requestedWasmFunctionIndex = process.env.HUNIN_CODE_WASM_FUNCTION_INDEX
  ? Number(process.env.HUNIN_CODE_WASM_FUNCTION_INDEX)
  : null;
const hash = (bytes: Buffer) =>
  createHash('sha256').update(bytes).digest('hex');
const diagnostics: string[] = [];
const seen = new Set<string>();
let moduleLimitReported = false;
let used = 0;
let custodyAttempted = false;
const session = new inspector.Session();
if (directory && Number.isSafeInteger(maximum) && maximum > 0) {
  // Enabling Debugger during the workload forces Wasm debug compilation.
  // Snapshot only still-live modules after its event loop has drained.
  process.once('beforeExit', () => {
    custodyAttempted = true;
    try {
      session.connect();
      session.on('Debugger.scriptParsed', ({ params }) => {
        const language = (params as typeof params & { scriptLanguage?: string })
          .scriptLanguage;
        if (language !== 'WebAssembly' && !params.url?.startsWith('wasm:'))
          return;
        if (seen.has(params.scriptId)) return;
        if (seen.size >= 1024) {
          if (!moduleLimitReported)
            diagnostics.push('Live Wasm module custody limit exceeded.');
          moduleLimitReported = true;
          return;
        }
        seen.add(params.scriptId);
        session.post(
          'Debugger.getScriptSource',
          { scriptId: params.scriptId },
          (error, result) => {
            if (error) {
              diagnostics.push('Runtime Wasm bytes could not be read.');
              return;
            }
            const bytecode = (result as { bytecode?: string }).bytecode;
            if (typeof bytecode !== 'string') {
              diagnostics.push('Runtime returned no Wasm bytecode.');
              return;
            }
            if (bytecode.length > Math.ceil((maximum - used) / 3) * 4) {
              diagnostics.push('Wasm custody byte budget exceeded.');
              return;
            }
            const bytes = Buffer.from(bytecode, 'base64');
            if (bytes.length > maximum - used) {
              diagnostics.push('Wasm custody byte budget exceeded.');
              return;
            }
            const digest = hash(bytes);
            const name = `wasm-${process.pid}-${threadId}-${digest}.wasm`;
            try {
              const path = join(directory, name);
              try {
                writeFileSync(path, bytes, { mode: 0o600, flag: 'wx' });
                used += bytes.length;
              } catch (cause) {
                if ((cause as NodeJS.ErrnoException).code !== 'EEXIST')
                  throw cause;
              }
              writeFileSync(
                `${path}.meta.json`,
                JSON.stringify({
                  path: name,
                  sha256: digest,
                  script_id: params.scriptId,
                  url: params.url,
                  pid: process.pid,
                  tid: String(threadId),
                  inspector_hash: params.hash,
                }),
                { mode: 0o600 },
              );
            } catch {
              diagnostics.push('Runtime Wasm bytes could not be retained.');
            }
          },
        );
      });
      session.post('Debugger.enable', {}, (error) => {
        if (error)
          diagnostics.push('Runtime Inspector does not provide Wasm custody.');
      });
    } catch {
      diagnostics.push(
        'Runtime Inspector setup failed; Wasm custody is unavailable.',
      );
    }
  });
  if (isMainThread)
    process.once('exit', () => {
      try {
        const entries = readdirSync(directory, { withFileTypes: true })
          .filter((entry) => entry.isFile())
          .map((entry) => entry.name);
        const logs = entries
          .filter((name) => name.endsWith(`-${process.pid}-code-v8.log`))
          .map((path) => ({
            path,
            ...(entries.includes(`${path}.ll`)
              ? { ll_path: `${path}.ll` }
              : {}),
          }));
        const wasm = entries
          .filter(
            (name) =>
              name.startsWith(`wasm-${process.pid}-`) &&
              name.endsWith('.wasm.meta.json'),
          )
          .flatMap((name) => {
            try {
              return [
                JSON.parse(
                  readFileSync(join(directory, name), 'utf8'),
                ) as unknown,
              ];
            } catch {
              diagnostics.push('A Wasm custody descriptor could not be read.');
              return [];
            }
          });
        const compiler_exports = entries
          .filter((name) => name.startsWith('turbo-') && name.endsWith('.json'))
          .map((path) => ({ path }));
        const bytes = entries.reduce(
          (sum, name) => sum + statSync(join(directory, name)).size,
          0,
        );
        if (bytes > maximum)
          diagnostics.push('Code capture artifact byte budget exceeded.');
        if (!custodyAttempted)
          diagnostics.push(
            'Target exited before the post-workload Wasm custody snapshot.',
          );
        writeFileSync(
          join(directory, `code-${process.pid}.v8code.json`),
          JSON.stringify({
            schema_version: 1,
            run_id: process.env.HUNIN_RUN_ID,
            pid: process.pid,
            runtime: {
              node: process.version,
              v8: process.versions.v8,
              arch: process.arch,
              platform: process.platform,
              exec_path: process.execPath,
              exec_argv: execArgv,
            },
            status: 'target_exit_observed',
            max_artifact_bytes: maximum,
            wasm_custody: 'post_workload_live_module_snapshot',
            ...(compilerExport
              ? {
                  published_code: { path: 'stdout.txt' },
                  compiler_scope: {
                    requested_wasm_function_index: requestedWasmFunctionIndex,
                    requested_scope:
                      requestedWasmFunctionIndex === null
                        ? 'all_wasm_functions'
                        : 'function_index_across_modules',
                  },
                }
              : {}),
            logs,
            wasm,
            compiler_exports,
            diagnostics: diagnostics.slice(0, 100),
          }),
          { mode: 0o600 },
        );
      } catch {
        process.stderr.write(
          'hunin: code capture inventory could not be retained\n',
        );
      }
    });
}
