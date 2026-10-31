/** Combine view lifetime with a replaceable read request; this never controls a job. */
export function linkedRequest(parent: AbortSignal, local: AbortSignal) {
  const controller = new AbortController();
  const abort = () => controller.abort();
  for (const signal of [parent, local]) {
    if (signal.aborted) abort();
    else signal.addEventListener('abort', abort, { once: true });
  }
  return {
    signal: controller.signal,
    finish: () => {
      parent.removeEventListener('abort', abort);
      local.removeEventListener('abort', abort);
    },
  };
}

/** Interactive foreground work stays linked until its owning view or selection ends. */
export function linkedLifetime(
  parent: AbortSignal,
  local: AbortSignal,
): AbortSignal {
  const lifetime = linkedRequest(parent, local);
  if (lifetime.signal.aborted) lifetime.finish();
  else
    lifetime.signal.addEventListener('abort', lifetime.finish, { once: true });
  return lifetime.signal;
}
