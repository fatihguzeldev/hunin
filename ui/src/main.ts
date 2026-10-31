import { investigate } from './investigation/workbench';
import { Workspace } from './workspace';
import { calls } from './views/calls';
import { profile } from './views/profile';
import { timeline } from './views/timeline';
import { resources } from './views/resources';
import { evidence } from './views/evidence';
import { source } from './source/view';
import { analyses } from './analysis/view';
import { comparison } from './views/comparison';
import './style.css';
import './design/components.css';
import './design/layout.css';
import { initializeTheme } from './design/theme';
import { selectProject } from './projects/selection';

initializeTheme();

const root = document.querySelector<HTMLDivElement>('#app')!;
void selectProject(root, (project) => {
  new Workspace(
    {
      investigate: { name: 'Investigate', render: investigate },
      timeline: { name: 'Timeline', render: timeline },
      profile: { name: 'Stack profile', render: profile },
      calls: { name: 'Function calls', render: calls },
      resources: { name: 'Resources', render: resources },
      evidence: { name: 'Evidence', render: evidence },
      analyses: { name: 'Analyzers', render: analyses },
      compare: { name: 'Compare', render: comparison },
    },
    source,
    project,
  ).mount(root);
});
