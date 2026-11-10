function selectedMainWork(n) {
  let sum = 0;
  for (let i = 0; i < n; i++) sum = (sum + i) >>> 0;
  return sum;
}
const workerSource = `function workerWork(n) {\n  let sum = 0;\n  for (let i = 0; i < n; i++) sum = (sum + i) >>> 0;\n  return sum;\n}\nonmessage = event => postMessage(workerWork(event.data));\npostMessage('ready');`;
const worker = new Worker(URL.createObjectURL(new Blob([workerSource], { type: 'text/javascript' })));
const other = window.open('browser-scope-other.html', 'unrelated-work');
let workerReady, workerResult, otherReady, otherResult;
const workerInitialized = new Promise(resolve => workerReady = resolve);
const workerCompleted = new Promise(resolve => workerResult = resolve);
const otherInitialized = new Promise(resolve => otherReady = resolve);
const otherCompleted = new Promise(resolve => otherResult = resolve);
worker.onmessage = ({ data }) => data === 'ready' ? workerReady() : workerResult(data);
window.addEventListener('message', event => {
  if (event.source !== other) return;
  event.data === 'ready' ? otherReady() : otherResult(event.data);
});
globalThis.run = async function runScopedWork(input) {
  if (!other) throw new Error('unrelated context was not created');
  await Promise.all([workerInitialized, otherInitialized]);
  const count = 50000000;
  worker.postMessage(count); other.postMessage(count, '*');
  const selected = selectedMainWork(count);
  const result = { selected, worker: await workerCompleted, unrelated: await otherCompleted };
  if (input?.terminateWorker) {
    worker.terminate();
    await new Promise(resolve => setTimeout(resolve, 30));
  }
  return result;
};
