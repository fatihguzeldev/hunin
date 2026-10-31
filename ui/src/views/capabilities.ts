import { operation } from '../api';
import { badge, el } from '../dom';

export async function capabilities(signal?: AbortSignal): Promise<HTMLElement> {
  const data = await operation('capabilities', {}, signal);
  signal?.throwIfAborted();
  return el(
    'div',
    { class: 'stack' },
    el(
      'div',
      { class: 'view-title' },
      el('h1', {}, 'Collection capabilities'),
      badge(`${data.host.os} · ${data.host.arch}`),
    ),
    el(
      'p',
      { class: 'muted' },
      'Installed collection methods and prerequisites. Discovery does not run a workload or verify a recording session.',
    ),
    data.native_sampling_permission
      ? el(
          'section',
          { class: 'panel panel-body stack' },
          el(
            'div',
            { class: 'inline' },
            el('h2', {}, 'Native sampling permission'),
            badge(
              data.native_sampling_permission.state,
              data.native_sampling_permission.state === 'prerequisites_ready'
                ? 'good'
                : 'warn',
            ),
          ),
          el(
            'p',
            { class: 'metric-note' },
            data.native_sampling_permission.probe,
          ),
        )
      : null,
    el(
      'div',
      { class: 'capability-grid' },
      ...data.runtimes.map((runtime) =>
        el(
          'section',
          { class: 'capability' },
          el(
            'div',
            { class: 'inline' },
            el('h2', {}, runtime.runtime),
            badge(
              runtime.available ? 'runtime available' : 'runtime unavailable',
              runtime.available ? 'good' : 'warn',
            ),
          ),
          runtime.version ? el('p', { class: 'mono' }, runtime.version) : null,
          el(
            'div',
            { class: 'inline' },
            ...(runtime.capabilities ?? []).map((value) => badge(value)),
          ),
          ...(runtime.modes ?? []).map((mode) =>
            el(
              'div',
              { class: 'result-item' },
              el(
                'div',
                { class: 'inline' },
                el('h3', {}, mode.mode),
                badge(
                  mode.availability,
                  mode.availability === 'prerequisites_ready' ? 'good' : 'warn',
                ),
              ),
              el(
                'details',
                {},
                el('summary', {}, 'Method details'),
                el(
                  'div',
                  { class: 'stack' },
                  el('p', {}, mode.mechanism),
                  el('p', { class: 'metric-note' }, mode.basis),
                  badge(
                    mode.recording_verified
                      ? 'Recording verified'
                      : 'Recording not checked',
                    mode.recording_verified ? 'good' : '',
                  ),
                ),
              ),
            ),
          ),
          ...runtime.diagnostics.map((message) => el('p', {}, message)),
          runtime.executable || runtime.verification
            ? el(
                'details',
                {},
                el('summary', {}, 'Runtime details'),
                runtime.executable
                  ? el('p', { class: 'mono source-label' }, runtime.executable)
                  : null,
                runtime.verification
                  ? el('p', { class: 'metric-note' }, runtime.verification)
                  : null,
              )
            : null,
        ),
      ),
    ),
  );
}
