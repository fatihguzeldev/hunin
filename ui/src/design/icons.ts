// 24-unit drawings, optically balanced for 18–20px controls.
// Keep the vocabulary quiet: rounded terminals, short chamfers, open counters.
const drawings = {
  search: '<circle cx="10.5" cy="10.5" r="6.25"/><path d="m15 15 4.75 4.75"/>',
  arrow: '<path d="M4.5 12h15m-5-5 5 5-5 5"/>',
  down: '<path d="m7 9.5 5 5 5-5"/>',
  folder:
    '<path d="M3.5 7V5.5h6l2 2h9v10.75l-1.25 1.25H4.75L3.5 18.25V7Zm0 3h17"/>',
  stacks: '<path d="m4 7.5 8-4 8 4-8 4-8-4Zm0 4.5 8 4 8-4M4 16.5l8 4 8-4"/>',
  recording:
    '<path d="M5.5 3.75h11.75l2.25 2.25v12.75L18 20.25H5.5L4 18.75V5.25l1.5-1.5Z"/><path d="M8 11h8M8 15h5"/><circle cx="8.25" cy="7.5" r=".75" fill="currentColor" stroke="none"/>',
  code: '<path d="m7.5 7.5-4.5 4.5 4.5 4.5m9-9 4.5 4.5-4.5 4.5M14 4.5l-4 15"/>',
  check: '<path d="m5 12 4.5 4.5L19 7"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 3v1.5M12 19.5V21M3 12h1.5M19.5 12H21M5.6 5.6l1.05 1.05m10.7 10.7 1.05 1.05M5.6 18.4l1.05-1.05m10.7-10.7 1.05-1.05"/>',
  moon: '<path d="M19.5 15.25A8 8 0 0 1 8.75 4.5a8 8 0 1 0 10.75 10.75Z"/>',
  plus: '<path d="M5.5 12h13M12 5.5v13"/>',
  minus: '<path d="M5.5 12h13"/>',
  fit: '<path d="M8.5 4.5H5.75L4.5 5.75V8.5m11-4h2.75l1.25 1.25V8.5m-15 7v2.75l1.25 1.25H8.5m7 0h2.75l1.25-1.25V15.5M8 12h8"/>',
  close: '<path d="m6.5 6.5 11 11m-11 0 11-11"/>',
  link: '<path d="m9.25 14.75 5.5-5.5m-3.25-2 .75-.75a4.25 4.25 0 0 1 6 6l-2.5 2.5a4.25 4.25 0 0 1-6 0m2.75 2-.75.75a4.25 4.25 0 0 1-6-6l2.5-2.5a4.25 4.25 0 0 1 6 0"/>',
  external:
    '<path d="M13.5 4.5h6v6m0-6-9 9M9 4.5H6L4.5 6v12L6 19.5h12l1.5-1.5v-3"/>',
  back: '<path d="M19.5 12h-15m5-5-5 5 5 5"/>',
  signal: '<path d="M3 13.5h3l3-6 4.5 10 3-7 2.5 3h2"/>',
  range: '<path d="M7 6.5H4.5v11H7m10-11h2.5v11H17M8.5 12h7"/>',
  focus:
    '<path d="M8 4.5H5.75L4.5 5.75V8m11.5-3.5h2.25l1.25 1.25V8m-15 8v2.25l1.25 1.25H8m8 0h2.25l1.25-1.25V16"/><path d="M9 9h6v6H9Z"/>',
  inspect:
    '<path d="M13 4.5H6L4.5 6v12L6 19.5h7m1.5-15h3l1.5 1.5v12L18 19.5h-3M14.5 4.5v15M8 9h3M8 12h3M8 15h2"/>',
  settings:
    '<path d="M4 7h6m4 0h6M4 17h10m4 0h2"/><circle cx="12" cy="7" r="2"/><circle cx="16" cy="17" r="2"/>',
  previous: '<path d="M5 6v12m13-11-6 5 6 5"/>',
  next: '<path d="M19 6v12M6 7l6 5-6 5"/>',
  command:
    '<path d="M8.5 8.5h7v7h-7Zm0 0H6.25A2.25 2.25 0 1 1 8.5 6.25V8.5Zm7 0V6.25a2.25 2.25 0 1 1 2.25 2.25H15.5Zm0 7h2.25a2.25 2.25 0 1 1-2.25 2.25V15.5Zm-7 0v2.25a2.25 2.25 0 1 1-2.25-2.25H8.5Z"/>',
  'chevron-right': '<path d="m9.5 7 5 5-5 5"/>',
  grip: '<path d="M9 6h.01M15 6h.01M9 12h.01M15 12h.01M9 18h.01M15 18h.01" stroke-width="2.5"/>',
  reset: '<path d="M4.5 9a8 8 0 1 1 .25 6.5M4.5 4.5V9H9"/>',
  expand: '<path d="M14 4.5h5.5V10m0-5.5-6 6M10 19.5H4.5V14m0 5.5 6-6"/>',
} as const;

export type IconName = keyof typeof drawings;

/** Icon geometry is fixed application code; workload content never enters SVG markup. */
export function icon(name: IconName, className = ''): SVGSVGElement {
  const markup = `<svg xmlns="http://www.w3.org/2000/svg">${drawings[name]}</svg>`;
  const parsed = new DOMParser().parseFromString(markup, 'image/svg+xml');
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  for (const child of parsed.documentElement.children)
    svg.append(document.importNode(child, true));
  for (const [key, value] of Object.entries({
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    'stroke-width': '1.5',
    'stroke-linecap': 'round',
    'stroke-linejoin': 'round',
    'aria-hidden': 'true',
    focusable: 'false',
    class: `icon ${className}`.trim(),
  }))
    svg.setAttribute(key, value);
  return svg;
}
