import { operation } from '../api';
import { badge, el } from '../dom';

export async function capabilities(): Promise<HTMLElement> {
  const data = await operation('capabilities', {});
  return el('div', { class: 'stack' },
    el('div', { class: 'view-title' }, el('h1', {}, 'Collection capabilities'), badge(`${data.host.os} · ${data.host.arch}`)),
    el('p', { class: 'muted' }, 'Discovery checks runtime and collection prerequisites. It does not record a workload or prove that a sampling session can attach to the selected target.'),
    data.native_sampling_permission ? el('section', { class: 'panel panel-body stack' },
      el('div', { class: 'inline' }, el('h2', {}, 'Native sampling permission'), badge(data.native_sampling_permission.state, data.native_sampling_permission.state === 'prerequisites_ready' ? 'good' : 'warn')),
      el('p', { class: 'metric-note' }, data.native_sampling_permission.probe)) : null,
    el('div', { class: 'capability-grid' }, ...data.runtimes.map(runtime =>
      el('section', { class: 'capability' }, el('div', { class: 'inline' }, el('h2', {}, runtime.runtime), badge(runtime.available ? 'runtime probe passed' : 'runtime unavailable', runtime.available ? 'good' : 'warn')),
        runtime.version ? el('p', { class: 'mono' }, runtime.version) : null,
        runtime.executable ? el('p', { class: 'mono source-label' }, runtime.executable) : null,
        runtime.verification ? el('p', { class: 'metric-note' }, runtime.verification) : null,
        el('div', { class: 'inline' }, ...(runtime.capabilities ?? []).map(value => badge(value))),
        ...(runtime.modes ?? []).map(mode => el('div', { class: 'result-item' },
          el('div', { class: 'inline' }, el('h3', {}, mode.mode), badge(mode.availability, mode.availability === 'prerequisites_ready' ? 'good' : 'warn')),
          el('p', {}, mode.mechanism), el('p', { class: 'metric-note' }, mode.basis), badge(mode.recording_verified ? 'recording verified' : 'recording not verified by discovery', mode.recording_verified ? 'good' : 'warn'))),
        ...runtime.diagnostics.map(message => el('p', {}, message))))));
}
