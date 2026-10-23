import { operation } from '../api';
import { badge, el } from '../dom';

export async function capabilities(): Promise<HTMLElement> {
  const data = await operation('capabilities', {});
  return el('div', { class: 'stack' },
    el('div', { class: 'view-title' }, el('h1', {}, 'Collection capabilities'), badge(`${data.host.os} · ${data.host.arch}`)),
    el('p', { class: 'muted' }, 'Availability is checked on this host. A runtime being installed does not establish every collection mode.'),
    el('div', { class: 'capability-grid' }, ...data.runtimes.map(runtime =>
      el('section', { class: 'capability' }, el('div', { class: 'inline' }, el('h2', {}, runtime.runtime), badge(runtime.available ? 'available' : 'unavailable', runtime.available ? 'good' : 'warn')),
        runtime.version ? el('p', { class: 'mono' }, runtime.version) : null,
        el('div', { class: 'inline' }, ...runtime.capabilities.map(value => badge(value))),
        ...runtime.diagnostics.map(message => el('p', {}, message))))));
}
