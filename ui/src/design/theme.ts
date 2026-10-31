import { button } from '../dom';
import { icon } from './icons';

type Theme = 'light' | 'dark';
const key = 'hunin.theme';

export function initializeTheme(): void {
  let theme: Theme = 'light';
  try {
    const saved = localStorage.getItem(key);
    if (saved === 'light' || saved === 'dark') theme = saved;
  } catch {
    /* Theme remains usable without private storage. */
  }
  document.documentElement.dataset.theme = theme;
}

export function themeControl(): HTMLButtonElement {
  const control = button(
    '',
    () => {
      const theme: Theme =
        document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
      document.documentElement.dataset.theme = theme;
      try {
        localStorage.setItem(key, theme);
      } catch {
        /* In-memory selection remains usable. */
      }
      render();
    },
    'button subtle icon-button theme-control',
  );
  function render() {
    const dark = document.documentElement.dataset.theme === 'dark';
    const label = `Switch to ${dark ? 'light' : 'dark'} theme`;
    control.setAttribute('aria-label', label);
    control.setAttribute('title', label);
    control.setAttribute('aria-pressed', String(dark));
    control.replaceChildren(icon(dark ? 'sun' : 'moon'));
  }
  render();
  return control;
}
