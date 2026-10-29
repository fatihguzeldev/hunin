import { Workspace } from './workspace';
import { profile } from './views/profile';
import { timeline } from './views/timeline';
import { resources } from './views/resources';
import { evidence, source } from './views/evidence';
import { analyses } from './views/analyses';
import { comparison } from './views/comparison';
import './style.css';

new Workspace(
  {
    timeline: { name: 'Timeline', render: timeline },
    profile: { name: 'Stack profile', render: profile },
    resources: { name: 'Resources', render: resources },
    evidence: { name: 'Evidence', render: evidence },
    analyses: { name: 'Analyzers', render: analyses },
    compare: { name: 'Compare', render: comparison },
  },
  source,
).mount(document.querySelector<HTMLDivElement>('#app')!);
