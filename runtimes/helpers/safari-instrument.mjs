import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
import crypto from 'node:crypto';

const [input, output, metadataPath, babelRoot] = process.argv.slice(2);
if (![input, output, metadataPath, babelRoot].every(value => value && path.isAbsolute(value))) throw new Error('instrumentation paths and Babel root must be absolute');
const require = createRequire(path.join(babelRoot, 'package.json'));
const babel = await import(pathToFileURL(require.resolve('@babel/core')).href);
if (babel.version !== '8.1.0') throw new Error(`expected Babel 8.1.0, got ${babel.version}`);
const inputDescriptor = fs.openSync(input,fs.constants.O_RDONLY|(fs.constants.O_NOFOLLOW||0)|(fs.constants.O_NONBLOCK||0));
let bytes;
try {
 const stat=fs.fstatSync(inputDescriptor);if(!stat.isFile()||stat.size>4*1024*1024)throw new Error('instrumentation input must be a regular file within 4 MiB');
 const buffer=Buffer.alloc(4*1024*1024+1);let used=0;
 while(used<buffer.length){const n=fs.readSync(inputDescriptor,buffer,used,buffer.length-used,null);if(!n)break;used+=n;}
 if(used>4*1024*1024)throw new Error('instrumentation input exceeds 4 MiB');bytes=buffer.subarray(0,used);
}finally{fs.closeSync(inputDescriptor);}
const source = bytes.toString('utf8');
if (!Buffer.from(source).equals(bytes)) throw new Error('instrumentation source must be UTF-8');
const digest = value => crypto.createHash('sha256').update(value).digest('hex');
const originalHash = digest(bytes);
const transformId = digest(`hunin-safari-body-events-v1\0babel8.1.0\0${originalHash}\0${input}`);
const functions = [];
let hook;
let strictProgram=false;
const transformed = babel.transformSync(source, {
  filename: input, sourceType: 'unambiguous', configFile: false, babelrc: false, comments: true,
  plugins: [({ types: t }) => ({ visitor: {
    Program: { enter(p) {
      if (p.scope.getBinding('globalThis')) throw p.buildCodeFrameError('a program-level globalThis binding needs a separate instrumentation path');
      strictProgram=p.node.sourceType==='module'||p.node.directives.some(item=>item.value.value==='use strict');
      hook = p.scope.generateUidIdentifier('huninObservation'+transformId.slice(0,12));
    } },
    WithStatement(p) { throw p.buildCodeFrameError('with statements need a separate instrumentation path'); },
    CallExpression(p) { if (t.isIdentifier(p.node.callee, {name:'eval'}) && !p.scope.getBinding('eval')) throw p.buildCodeFrameError('direct eval needs a separate instrumentation path'); },
    Function: { exit(p) {
      const node = p.node;
      if (!node.body || !node.loc) return;
      if (functions.length >= 10000) throw p.buildCodeFrameError('function declaration budget exceeded');
      const ordinal = functions.length;
      let name = node.id?.name;
      if (!name && node.key) name = !node.computed && t.isIdentifier(node.key) ? node.key.name : t.isStringLiteral(node.key) ? node.key.value : '[computed]';
      if (!name && p.parentPath.isVariableDeclarator() && t.isIdentifier(p.parentPath.node.id)) name = p.parentPath.node.id.name;
      if (!name && p.parentPath.isAssignmentExpression() && t.isMemberExpression(p.parentPath.node.left) && !p.parentPath.node.left.computed && t.isIdentifier(p.parentPath.node.left.property)) name = p.parentPath.node.left.property.name;
      name ||= '(anonymous)';
      functions.push({ordinal,name,kind:node.type,async:Boolean(node.async),generator:Boolean(node.generator),line:node.loc.start.line-1,column:node.loc.start.column,end_line:node.loc.end.line-1,end_column:node.loc.end.column});
      if (!t.isBlockStatement(node.body)) node.body = t.blockStatement([t.returnStatement(node.body)]);
      const original = node.body.body;
      const call = p.scope.generateUidIdentifier('huninCall');
      const thrown = p.scope.generateUidIdentifier('huninThrown');
      const error = p.scope.generateUidIdentifier('huninError');
      node.body.body = [
        t.variableDeclaration('const',[t.variableDeclarator(call,t.callExpression(t.memberExpression(hook,t.identifier('enter')),[t.numericLiteral(ordinal)]))]),
        t.variableDeclaration('let',[t.variableDeclarator(thrown,t.booleanLiteral(false))]),
        t.tryStatement(t.blockStatement(original),t.catchClause(error,t.blockStatement([t.expressionStatement(t.assignmentExpression('=',thrown,t.booleanLiteral(true))),t.throwStatement(error)])),t.blockStatement([t.expressionStatement(t.callExpression(t.memberExpression(hook,t.identifier('exit')),[call,thrown]))])),
      ];
    } },
  } })],
});

const runtime = `;(() => {
 const g=globalThis;
 if(g.__HuninObservation){if(g.__HuninObservation.format!=='hunin.safari-instrument-v1')throw Error('observer namespace collision');return;}
 const now=g.performance.now.bind(g.performance),origin=g.performance.timeOrigin;
 const push=Function.prototype.call.bind(Array.prototype.push);
 const set=Function.prototype.call.bind(Map.prototype.set),get=Function.prototype.call.bind(Map.prototype.get),del=Function.prototype.call.bind(Map.prototype.delete),clear=Function.prototype.call.bind(Map.prototype.clear);
 const scripts=[],definitions=[],active=new Map(),registrations=new Map();let events=[],capturing=false,nextScript=1,nextCall=1,generation=0,maxEvents=100000,lost=0,errors=0,started=0,firstLoss=null;
 function lostEvent(){lost++;if(firstLoss===null)firstLoss=now();}
 function enter(functionId){try{if(!capturing)return 0;if(events.length>=maxEvents||!Number.isSafeInteger(nextCall)){lostEvent();return -generation;}const id=nextCall++;set(active,id,functionId);push(events,{call_id:id,function_id:functionId,kind:'entry',time_ms:now(),body_throw:null});return id;}catch(e){errors++;return 0;}}
 function exit(id,thrown){try{if(id<0){if(capturing&&id===-generation)lostEvent();return;}if(!id)return;const fn=get(active,id);del(active,id);if(!capturing||fn===undefined)return;if(events.length>=maxEvents){lostEvent();return;}push(events,{call_id:id,function_id:fn,kind:'exit',time_ms:now(),body_throw:Boolean(thrown)});}catch(e){errors++;}}
 const api={format:'hunin.safari-instrument-v1',register(metadata){
  if(get(registrations,metadata.transform_id))return get(registrations,metadata.transform_id);
  if(scripts.length>=128||definitions.length+metadata.functions.length>10000)throw Error('script/function budget exceeded');
  const script='script-'+nextScript++;push(scripts,{id:script,transform_id:metadata.transform_id,source_name:metadata.source_name,original_sha256:metadata.original_sha256,instrumented_sha256:null});
  const ids=[];for(const fn of metadata.functions){const id=script+':'+fn.ordinal;ids[fn.ordinal]=id;push(definitions,{...fn,id,script});}
  const hooks=Object.freeze({enter(ordinal){return enter(ids[ordinal]);},exit});set(registrations,metadata.transform_id,hooks);return hooks;
 },start(options={}){
  if(capturing)throw Error('observer already active');
  if(!Number.isSafeInteger(generation+1))throw Error('capture generation exhausted');generation++;
  maxEvents=options.max_events??100000;if(!Number.isInteger(maxEvents)||maxEvents<1||maxEvents>100000)throw Error('invalid event budget');
  for(const script of scripts){const matching=(options.bindings||[]).filter(binding=>binding.transform_id===script.transform_id);if(!matching.length||matching.some(binding=>binding.instrumented_sha256!==matching[0].instrumented_sha256)||!/^[a-f0-9]{64}$/.test(matching[0].instrumented_sha256))throw Error('missing or ambiguous executed script binding');script.instrumented_sha256=matching[0].instrumented_sha256;}
  events=[];clear(active);lost=0;errors=0;firstLoss=null;started=now();capturing=true;return {active:true,format:'hunin.safari-instrument-v1',start_ms:started,max_events:maxEvents,script_count:scripts.length,function_count:definitions.length,bindings:scripts.map(script=>({script:script.id,transform_id:script.transform_id,instrumented_sha256:script.instrumented_sha256}))};
 },stop(){if(!capturing)throw Error('observer is not active');capturing=false;return {format:'hunin.safari-instrument-v1',time_origin_ms:origin,start_ms:started,end_ms:now(),scripts:scripts.map(value=>({...value})),functions:definitions.map(value=>({...value})),events:events.slice(),lost_events:lost,hook_errors:errors,first_loss_ms:firstLoss,open_calls:Array.from(active.keys()),semantics:{entry:'function body entry; generators enter on first resume',exit:'body completion or throw; returned promise settlement is not observed',elapsed:'wall spans include async/generator suspension and observer overhead',parentage:'no inferred synchronous or asynchronous parent',scope:'instrumented source functions after start; native/builtin code and bodies already active at start are outside scope'}};}}
 Object.defineProperty(g,'__HuninObservation',{value:Object.freeze(api),configurable:false,writable:false});
})();\n`;
const registration = {transform_id:transformId,source_name:input,original_sha256:originalHash,functions};
const generated = (strictProgram ? '"use strict";\n' : '') + runtime + `var ${hook.name}=globalThis.__HuninObservation.register(${JSON.stringify(registration)});\n` + transformed.code + '\n';
const metadata = {format:'hunin.safari-transform-v1',babel_version:'8.1.0',transform_id:transformId,source_name:input,original_sha256:originalHash,instrumented_sha256:digest(generated),functions,caveats:['observed body wall spans include suspension and observer overhead','no inferred async parent or operation-owned cpu','generator entry occurs on resume, not generator object creation','promise settlement after a returned thenable is not observed','source text reflection and exact stack locations change under instrumentation','direct eval, with, and a program-level globalThis binding are rejected']};
for (const [filename,value] of [[output,generated],[metadataPath,JSON.stringify(metadata)]]) {
 const descriptor=fs.openSync(filename,'wx',0o600);try{fs.writeFileSync(descriptor,value);fs.fsyncSync(descriptor);}finally{fs.closeSync(descriptor);}
}
