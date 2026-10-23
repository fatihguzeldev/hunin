import inspector from 'node:inspector';
import process from 'node:process';
import { appendFileSync, readFileSync, statSync, realpathSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join, relative, isAbsolute, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { threadId } from 'node:worker_threads';

const MAX_SOURCE = 2 * 1024 * 1024;
const MAX_MAP = 8 * 1024 * 1024;
const MAX_TOTAL = 32 * 1024 * 1024;
const hash = value => createHash('sha256').update(value).digest('hex');
let directory, root, used = 0, session;
const seen = new Set();
const diagnostic = message => process.stderr.write(`hunin: script source capture unavailable: ${message}\n`);
try {
  directory = process.env.HUNIN_JS_PROFILE_DIR;
  const requested = process.env.HUNIN_PROJECT_ROOT ?? process.cwd();
  try { root = realpathSync(requested); } catch { root = resolve(requested); }
} catch { /* The target may deny environment or filesystem access. */ }
function localPath(uri) {
  const url = new URL(uri);
  if (url.protocol !== 'file:') throw Error('only local project source files are read');
  let path = resolve(fileURLToPath(url));
  try { path = realpathSync(path); } catch { /* Scope check can precede a denied read. */ }
  const inside = relative(root, path);
  if (inside.startsWith('..') || isAbsolute(inside)) throw Error('source is outside the measured project');
  return path;
}
function read(uri, limit) {
  localPath(uri);
  const path = fileURLToPath(new URL(uri));
  if (statSync(path).size > limit) throw Error('source file exceeds the bounded capture limit');
  const content = readFileSync(path, 'utf8');
  if (Buffer.byteLength(content) > limit) throw Error('source file grew beyond the bounded capture limit');
  return content;
}
function save(entry) {
  const text = JSON.stringify(entry) + '\n';
  const size = Buffer.byteLength(text);
  if (used + size > MAX_TOTAL) return;
  appendFileSync(join(directory, `scripts-${process.pid}-${threadId}.jsonl`), text, { mode: 0o600 });
  used += size;
}
function sourceMap(reference, uri, source) {
  let mapUri, content;
  if (reference.startsWith('data:')) {
    const match = reference.match(/^data:application\/json(?:;charset=[^;,]+)?(;base64)?,(.*)$/s);
    if (!match) throw Error('unsupported inline source-map media type');
    mapUri = uri;
    content = match[1] ? Buffer.from(match[2], 'base64').toString('utf8') : decodeURIComponent(match[2]);
  } else {
    mapUri = new URL(reference, uri).href;
    content = read(mapUri, MAX_MAP);
  }
  if (Buffer.byteLength(content) > MAX_MAP) throw Error('source map exceeds8MiB');
  const map = JSON.parse(content);
  if (map.version !== 3) throw Error('unsupported source-map version');
  if (map.file && new URL(map.file, mapUri).href !== uri) throw Error('source-map file field disagrees with the observed script URL');
  const debugId = source.match(/(?:#|@)\s*debugId\s*=\s*([\w-]+)/)?.[1];
  if (debugId && map.debug_id && debugId.toLowerCase() !== map.debug_id.toLowerCase()) throw Error('script and source-map debug IDs disagree');
  const originals = [];
  function collect(map, base) {
    if (Array.isArray(map.sections)) {
      for (const section of map.sections) {
        if (!section.map) throw Error('external indexed source-map sections are not captured');
        collect(section.map, base);
      }
      return;
    }
    for (const [index, name] of (map.sources ?? []).entries()) {
      const prefix = map.sourceRoot ? map.sourceRoot.replace(/\/?$/, '/') : '';
      const sourceUri = new URL(prefix + name, base).href;
      const embedded = map.sourcesContent?.[index];
      let content, provenance;
      if (typeof embedded === 'string') { content = embedded; provenance = 'source_map_sourcesContent'; }
      else { try { content = read(sourceUri, MAX_SOURCE); provenance = 'filesystem_at_script_parse'; } catch { continue; } }
      if (Buffer.byteLength(content) <= MAX_SOURCE) originals.push({ uri: sourceUri, content, sha256: hash(content), provenance });
    }
  }
  collect(map, mapUri);
  return { uri: mapUri, content, sha256: hash(content), originals };
}
if (directory && root && !globalThis.Bun) {
  try {
    session = new inspector.Session();
    session.connect();
    session.on('Debugger.scriptParsed', ({ params }) => {
      if (!params.url?.startsWith('file:')) return;
      try { localPath(params.url); } catch { return; }
      const identity = `${params.scriptId}:${params.hash}`;
      if (seen.has(identity) || used >= MAX_TOTAL) return;
      seen.add(identity);
      session.post('Debugger.getScriptSource', { scriptId: params.scriptId }, (error, result) => {
        if (error || typeof result?.scriptSource !== 'string') return;
        try {
          const code = result.scriptSource;
          if (Buffer.byteLength(code) > MAX_SOURCE) return;
          const entry = { schema_version: 1, pid: process.pid, tid: String(threadId), runtime: globalThis.Deno ? 'deno' : 'node', script_id: params.scriptId, url: params.url,
            captured_at_us: Math.round((performance.timeOrigin + performance.now()) * 1000), generated_code: code, generated_sha256: hash(code), inspector_hash: params.hash,
            profile_position_basis: globalThis.Deno ? 'runtime_reported' : 'generated_javascript', source_map_url: params.sourceMapURL ?? '' };
          if (globalThis.Deno) {
            try { const content = read(params.url, MAX_SOURCE); entry.original = { uri: params.url, content, sha256: hash(content), provenance: 'filesystem_at_script_parse' }; } catch { /* Generated code remains available. */ }
          } else {
            const reference = params.sourceMapURL || [...code.matchAll(/(?:#|@)\s*sourceMappingURL\s*=\s*([^\s]+)/g)].at(-1)?.[1];
            if (reference) { try { entry.source_map = sourceMap(reference, params.url, code); } catch (error) { entry.source_map_error = error.message; } }
          }
          save(entry);
        } catch (error) { diagnostic(error.message); }
      });
    });
    session.post('Debugger.enable', error => { if (error) { diagnostic(error.message); session.disconnect(); } });
    process.once('beforeExit', () => session.disconnect());
  } catch (error) { diagnostic(error.message); }
}

// Bun reports TypeScript positions directly, but its local Node Inspector
// bridge does not emit Debugger.scriptParsed. Preserve that weaker basis.
export function captureBunProfile(profile, producer) {
  if (!directory || !root) return;
  for (const node of profile.nodes ?? []) {
    const call = node.callFrame;
    if (!call?.url?.startsWith('file:') || seen.has(call.scriptId)) continue;
    seen.add(call.scriptId);
    try {
      const content = read(call.url, MAX_SOURCE);
      save({ schema_version: 1, pid: process.pid, tid: String(threadId), runtime: 'bun', script_id: call.scriptId, url: call.url, profile_position_basis: 'runtime_reported',
        code_id: `bun:${producer}:${call.scriptId}`, original: { uri: call.url, content, sha256: hash(content), provenance: 'filesystem_at_profile_flush' } });
    } catch { /* Missing source never turns into a current-file mapping claim. */ }
  }
}
