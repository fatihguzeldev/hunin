import * as h from '../index.mjs';

await Promise.all(
  ['a', 'b'].map((name) =>
    h.withSpan(name, async () => {
      await new Promise((resolve) => setTimeout(resolve, name === 'a' ? 2 : 1));
      await h.withSpan(name + '-child', async () => h.counter('items', 1, 'count'));
    }),
  ),
);
h.flush();
