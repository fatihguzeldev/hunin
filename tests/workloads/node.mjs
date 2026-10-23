import { withContext, withSpan, counter } from '../../sdk/javascript/index.mjs';

function hotLoop() {
  let checksum = 0;
  for (let i = 0; i < 5_000_000; i++) checksum = (checksum + Math.sqrt(i)) % 1000;
  return checksum;
}

withContext('reference workload', () => withSpan('compute', () => {
  const value = hotLoop();
  counter('completed_items', 1);
  console.log(JSON.stringify({ ok: Number.isFinite(value), checksum: value }));
}));
