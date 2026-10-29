import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve, basename, join, delimiter } from 'node:path';
import { randomUUID } from 'node:crypto';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

/** Rust's canonical Win32 paths are not accepted by every runtime resolver. */
export function toolPath(
  value: string,
  platform: NodeJS.Platform = process.platform,
) {
  if (platform !== 'win32') return value;
  if (/^\\\\\?\\[a-z]:\\/i.test(value)) return value.slice(4);
  if (/^\\\\\?\\UNC\\/i.test(value)) return '\\\\' + value.slice(8);
  return value;
}

export interface CaptureRequest {
  command: string[];
  cwd: string;
  artifact_dir: string;
  run_id: string;
  runtime?: string;
  env?: Record<string, string>;
}

export type CaptureOutput =
  | { kind: 'profiles'; format: 'v8-cpuprofile' }
  | { kind: 'deno-workers'; producer_id: string }
  | { kind: 'bun-producers'; session_id: string; root_registration_id: string };

export function plan(request: CaptureRequest) {
  const { command, cwd, artifact_dir, run_id } = request;
  if (
    !Array.isArray(command) ||
    !command.length ||
    command.some((value) => typeof value !== 'string')
  )
    throw Error('command must be a nonempty array of strings');
  const targetEnv = request.env === undefined ? {} : request.env;
  if (
    !targetEnv ||
    typeof targetEnv !== 'object' ||
    Array.isArray(targetEnv) ||
    Object.entries(targetEnv).some(
      ([key, value]) =>
        !key ||
        /[=\0]/.test(key) ||
        typeof value !== 'string' ||
        value.includes('\0'),
    )
  )
    throw Error('env must be an object of valid string environment overrides');
  const effectiveEnv = { ...process.env, ...targetEnv };
  const requestedRuntime = (
    request.runtime ?? basename(command[0]).replace(/\.exe$/i, '')
  ).toLowerCase();
  const runtime = ['nodejs', 'javascript', 'typescript'].includes(
    requestedRuntime,
  )
    ? 'node'
    : requestedRuntime;
  if (!['node', 'bun', 'deno'].includes(runtime))
    throw Error(`unsupported JavaScript runtime: ${runtime}`);
  const directory = toolPath(resolve(artifact_dir));
  const project = toolPath(resolve(cwd));
  const executable = toolPath(command[0]);
  mkdirSync(directory, { recursive: true });
  const result = spawnSync(executable, ['--version'], {
    cwd: project,
    env: effectiveEnv,
    encoding: 'utf8',
    timeout: 10000,
  });
  if (result.status !== 0)
    throw Error(
      `cannot inspect ${runtime}: ${result.error?.message ?? result.stderr}`,
    );
  const runtime_version = result.stdout.trim().split('\n')[0];
  const version = runtime_version.match(/(\d+)\.(\d+)/);
  const major = Number(version?.[1]),
    minor = Number(version?.[2]);
  if (runtime === 'node' && major < 20)
    throw Error('Node 20 or newer is required');
  if (runtime === 'bun' && (major < 1 || (major === 1 && minor < 4)))
    throw Error('Bun 1.4 or newer is required for per-worker profiling');
  if (runtime === 'deno' && (major < 2 || (major === 2 && minor < 8)))
    throw Error('Deno 2.8 or newer is required for built-in CPU profiling');
  const env: Record<string, string> = {
    ...targetEnv,
    HUNIN_RUN_ID: run_id,
    HUNIN_JS_PROFILE_DIR: directory,
    HUNIN_PROJECT_ROOT: project,
  };
  const session_identity: Record<string, string> = {};
  let output: CaptureOutput = { kind: 'profiles', format: 'v8-cpuprofile' };
  let collector_identity:
    | { kind: string; measurement_contract: string }
    | undefined;
  const diagnostics = [];
  diagnostics.push(
    'JavaScript source capture retains bounded project code/map snapshots. Node and Deno enable Inspector Debugger for script identity; this instrumentation contributes overhead. Source maps describe compiler output and do not prove compiler correctness.',
  );
  let launched: string[];
  if (runtime === 'bun') {
    const sessionId = randomUUID();
    const rootRegistrationId = randomUUID();
    session_identity['bun-session'] = sessionId;
    session_identity['bun-root'] = rootRegistrationId;
    env.HUNIN_BUN_SESSION = sessionId;
    env.HUNIN_BUN_ROOT_REGISTRATION = rootRegistrationId;
    output = {
      kind: 'bun-producers',
      session_id: sessionId,
      root_registration_id: rootRegistrationId,
    };
    collector_identity = {
      kind: 'bun-inspector',
      measurement_contract: 'bun-producers-v1',
    };
    writeFileSync(
      join(directory, `bun-session-${sessionId}.json`),
      JSON.stringify({
        schema_version: 1,
        runtime: 'bun',
        run_id,
        session_id: sessionId,
        root_registration_id: rootRegistrationId,
        status: 'planned',
      }),
      { mode: 0o600, flag: 'wx' },
    );
    const preload = toolPath(
      fileURLToPath(new URL('../bun-preload.mjs', import.meta.url)),
    );
    // BUN_OPTIONS retains quote characters in --preload= values. Resolve a
    // whitespace-free unique shim through NODE_PATH, whose path list supports
    // spaces, then let ESM resolve the real file URL on every host.
    const shim = `hunin-bun-preload-${randomUUID()}.mjs`;
    session_identity['bun-preload'] = shim;
    writeFileSync(
      join(directory, shim),
      `import ${JSON.stringify(pathToFileURL(preload).href)};\n`,
      { mode: 0o600 },
    );
    env.NODE_PATH = [directory, effectiveEnv.NODE_PATH]
      .filter(Boolean)
      .join(delimiter);
    env.BUN_OPTIONS =
      `${effectiveEnv.BUN_OPTIONS ?? ''} --preload=${shim}`.trim();
    launched = [executable, '--preload', preload, ...command.slice(1)];
    diagnostics.push(
      'Bun workers are sampled through Inspector preloads; worker termination waits up to 100ms for a profiling flush. Forced process termination can leave the current segment incomplete.',
    );
  } else if (runtime === 'node') {
    const preload = toolPath(
      fileURLToPath(new URL('../clock-preload.mjs', import.meta.url)),
    );
    env.NODE_OPTIONS =
      `${effectiveEnv.NODE_OPTIONS ?? ''} --cpu-prof --cpu-prof-dir=${JSON.stringify(directory)} --import=${JSON.stringify(pathToFileURL(preload).href)}`.trim();
    launched = [executable, ...command.slice(1)];
  } else {
    const preload = toolPath(
      fileURLToPath(new URL('../clock-preload.mjs', import.meta.url)),
    );
    // Keep the target's explicit permissions. Hunin only adds access needed by its preload.
    const inspectorDenied = command.some(
      (value) =>
        value === '--deny-sys' ||
        (value.startsWith('--deny-sys=') &&
          value.slice('--deny-sys='.length).split(',').includes('inspector')),
    );
    const producer = randomUUID();
    output = { kind: 'deno-workers', producer_id: producer };
    session_identity['worker-session'] = producer;
    env.HUNIN_DENO_WORKER_SESSION = producer;
    writeFileSync(
      join(directory, `deno-workers-${producer}.jsonl`),
      JSON.stringify({
        schema_version: 1,
        runtime: 'deno',
        run_id,
        producer_id: producer,
        status: 'planned',
      }) + '\n',
      { mode: 0o600, flag: 'wx' },
    );
    const index = ['run', 'eval', 'test', 'bench'].includes(command[1]) ? 2 : 1;
    launched = [
      executable,
      ...command.slice(1, index),
      '--cpu-prof',
      `--cpu-prof-dir=${directory}`,
      '--preload',
      preload,
      `--allow-env=HUNIN_JS_PROFILE_DIR,HUNIN_PROJECT_ROOT,HUNIN_RUN_ID,HUNIN_RECORD_DIR,HUNIN_DENO_WORKER_SESSION`,
      ...(inspectorDenied ? [] : ['--allow-sys=inspector']),
      `--allow-read=${directory}`,
      `--allow-write=${directory}`,
      ...command.slice(index),
    ];
    if (inspectorDenied)
      diagnostics.push(
        'The target denies Inspector access; CPU samples retain an unmapped runtime clock when epoch calibration is unavailable.',
      );
    diagnostics.push(
      'Deno TypeScript CPU profile positions refer to generated JavaScript when the runtime omits source maps. Worker profiles are emitted by Deno itself. Child Deno processes need their own collection plan.',
    );
    diagnostics.push(
      'Deno main-isolate Web Workers have a bounded exit barrier for their exact profile files. Nested workers and node:worker_threads created outside this constructor are not enumerated by this barrier; forced process exit can leave collection incomplete.',
    );
  }
  return {
    command: launched,
    env,
    session_identity,
    output,
    collector_identity,
    diagnostics,
    runtime_version,
  };
}
