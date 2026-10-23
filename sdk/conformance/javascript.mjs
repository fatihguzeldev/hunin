import * as h from '../javascript/index.mjs';
import process from 'node:process';
import { createInterface } from 'node:readline/promises';

const child = process.argv.includes('--child');
if (child) {
  const token = JSON.parse(process.env.HUNIN_SDK_TOKEN || 'null');
  h.withContext(token, () => h.withSpan('remote', span => {
    h.counter('jobs', 1);
    console.log('HUNIN_REMOTE ' + (span?.id ?? ''));
  }));
} else {
  await h.withContext('request', () => h.withSpan('root', async root => {
    await Promise.all(['a', 'b'].map(name => h.withSpan(name, async () => {
      await new Promise(resolve => setTimeout(resolve, 1));
      h.withSpan(name + '-child', () => h.counter('jobs', 1));
    })));
    console.log('HUNIN_TOKEN ' + JSON.stringify(h.captureContext() ?? {}));
    const input = createInterface({ input: process.stdin });
    const remote = await input.question('');
    input.close();
    if (root?.id && remote) h.relation(root.id, remote, 'dispatches');
  }));
}
h.flush();
if (h.lastError()) throw h.lastError();
