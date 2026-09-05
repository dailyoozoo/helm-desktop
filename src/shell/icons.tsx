import type { CSSProperties } from 'react';
import { createElement, type ReactElement } from 'react';

/* ============================================================
   图标注册表 —— 视觉真值逐字取自 prototype/assets/app.js 的 P 表
   （Lucide 子集手工修订版；stroke 1.8 / round / currentColor）。
   2026-08-23 二次反馈：此前用 lucide-react 组件渲染，几何与原型
   不一致（如 server：原型为内缩双矩形+右侧短横线，lucide 为通宽
   矩形+左侧圆点），现统一改为原型路径，保证像素级一致。
   ============================================================ */
const ICON_PATHS: Record<string, string> = {
  helm: '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="2.5"/><path d="M12 2v7.5M12 14.5V22M2 12h7.5M14.5 12H22M5 5l5.23 5.23M13.77 13.77 19 19M19 5l-5.23 5.23M10.23 13.77 5 19"/>',
  home: '<path d="M4 11 12 4l8 7"/><path d="M6 9.5V20h12V9.5"/><path d="M10 20v-5h4v5"/>',
  chat: '<path d="M20.5 12a8 8 0 0 1-11.3 7.3L4 20.5l1.2-4.7A8 8 0 1 1 20.5 12z"/>',
  history:
    '<path d="M3.2 12a8.8 8.8 0 1 0 2.9-6.5"/><path d="M3 4.5V9h4.5"/><path d="M12 8v4.2l3 1.8"/>',
  server:
    '<rect x="3.5" y="4" width="17" height="7" rx="2"/><rect x="3.5" y="13" width="17" height="7" rx="2"/><circle cx="7.5" cy="7.5" r="1"/><circle cx="7.5" cy="16.5" r="1"/><path d="M14 7.5h3M14 16.5h3"/>',
  folder:
    '<path d="M3.5 7a2 2 0 0 1 2-2h2.8l2 2.2h6.2a2 2 0 0 1 2 2v7.6a2 2 0 0 1-2 2h-11a2 2 0 0 1-2-2z"/>',
  settings:
    '<circle cx="12" cy="12" r="3.1"/><path d="M12 2.6v2.6M12 18.8v2.6M21.4 12h-2.6M5.2 12H2.6M18.4 5.6 16.6 7.4M7.4 16.6 5.6 18.4M18.4 18.4l-1.8-1.8M7.4 7.4 5.6 5.6"/>',
  brain:
    '<path d="M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .94 7.863A3 3 0 1 0 9.002 22h6a3 3 0 1 0 4.585-3.242 4 4 0 0 0 .94-7.863A3 3 0 1 0 12 5Z"/><path d="M12 5v17M9 8h1M9 12h1M9 16h1M14 8h1M14 12h1M14 16h1"/>',
  slidershorizontal:
    '<path d="M21 4H14M10 4H3M21 12h-9M8 12H3M21 20h-5M12 20H3"/><path d="M14 2v4M8 10v4M16 18v4"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.4-3.4"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  send: '<path d="M12 19V5M6 11l6-6 6 6"/>',
  clip: '<path d="M20.5 11.5 12 20a4.6 4.6 0 0 1-6.5-6.5l8.4-8.4a3 3 0 0 1 4.3 4.3l-8.4 8.4a1.4 1.4 0 0 1-2-2l7.7-7.7"/>',
  down: '<path d="m6 9 6 6 6-6"/>',
  chevrondown: '<path d="m6 9 6 6 6-6"/>',
  right: '<path d="m9 6 6 6-6 6"/>',
  left: '<path d="m15 6-6 6 6 6"/>',
  up: '<path d="m6 15 6-6 6 6"/>',
  check: '<path d="m5 12 5 5L20 6"/>',
  x: '<path d="M6 6 18 18M18 6 6 18"/>',
  minus: '<path d="M5 12h14"/>',
  square: '<rect x="5" y="5" width="14" height="14" rx="1.5"/>',
  terminal:
    '<rect x="2.5" y="4" width="19" height="16" rx="3"/><path d="m7 9.5 3 2.5-3 2.5"/><path d="M13 15h4"/>',
  file: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>',
  gitbranch:
    '<circle cx="6" cy="6" r="2.3"/><circle cx="6" cy="18" r="2.3"/><circle cx="17.5" cy="7" r="2.3"/><path d="M6 8.3v7.4"/><path d="M17.5 9.3c0 4.2-3.4 4.7-6.2 4.7"/>',
  cpu: '<rect x="6.5" y="6.5" width="11" height="11" rx="2"/><rect x="9.6" y="9.6" width="4.8" height="4.8" rx="1"/><path d="M9.5 3v3M14.5 3v3M9.5 18v3M14.5 18v3M3 9.5h3M3 14.5h3M18 9.5h3M18 14.5h3"/>',
  zap: '<path d="M13 2.5 4.5 13.5H11l-1 8 8.5-11.5H12z"/>',
  sparkles:
    '<path d="m12 3 1.7 4.6L18 9.3l-4.3 1.7L12 15.6l-1.7-4.6L6 9.3l4.3-1.7z"/><path d="m18.5 14 .7 1.9 1.9.7-1.9.7-.7 1.9-.7-1.9-1.9-.7 1.9-.7z"/>',
  shield:
    '<path d="M12 3 5 5.8v5.2c0 4.4 3 7.4 7 8.9 4-1.5 7-4.5 7-8.9V5.8z"/><path d="m9.2 11.7 1.9 1.9 3.7-4"/>',
  plug: '<path d="M12 22v-5M9 8V2M15 8V2M18 8v5a6 6 0 0 1-12 0V8Z"/>',
  palette:
    '<path d="M12 3a9 9 0 1 0 0 18c1.3 0 1.8-1 1.8-1.9 0-1.4 1-1.9 2.1-1.9H17a4 4 0 0 0 4-4C21 7.3 17 3 12 3z"/><circle cx="7.5" cy="11" r="1"/><circle cx="12" cy="7.8" r="1"/><circle cx="16.3" cy="11" r="1"/>',
  keyboard:
    '<rect x="3" y="6" width="18" height="12" rx="2.5"/><path d="M7 10h.01M11 10h.01M15 10h.01M17 10h.01M8.5 14h7"/>',
  sliders:
    '<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2.2"/><circle cx="9" cy="17" r="2.2"/>',
  more: '<circle cx="5" cy="12" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="19" cy="12" r="1.5"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2.5"/><path d="M5 15a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2"/>',
  thumbsup:
    '<path d="M7 10v10H4a2 2 0 0 1-2-2v-6a2 2 0 0 1 2-2z"/><path d="M7 20h9.4a2 2 0 0 0 1.9-1.4l2-6A2 2 0 0 0 18.4 10H14l.7-3.3A2.7 2.7 0 0 0 12 3.5L7 10z"/>',
  thumbsdown:
    '<path d="M7 14V4H4a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2z"/><path d="M7 4h9.4a2 2 0 0 1 1.9 1.4l2 6a2 2 0 0 1-1.9 2.6H14l.7 3.3A2.7 2.7 0 0 1 12 20.5L7 14z"/>',
  helpcircle:
    '<circle cx="12" cy="12" r="10"/><path d="M9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3"/><path d="M12 17h.01"/>',
  play: '<path d="M7 5 19 12 7 19z"/>',
  stop: '<rect x="6.5" y="6.5" width="11" height="11" rx="2.5"/>',
  octagon: '<path d="M7.86 2h8.28L22 7.86v8.28L16.14 22H7.86L2 16.14V7.86z"/>',
  lock: '<rect x="4.5" y="10.5" width="15" height="10.5" rx="2.5"/><path d="M8 10.5V7a4 4 0 0 1 8 0v3.5"/><circle cx="12" cy="15.5" r="1.3"/>',
  refresh:
    '<path d="M20 11a8 8 0 0 0-13.8-5L4 8"/><path d="M4 3.5V8h4.5"/><path d="M4 13a8 8 0 0 0 13.8 5L20 16"/><path d="M20 20.5V16h-4.5"/>',
  edit: '<path d="M4 20h4.2L19 9.2a2 2 0 0 0-2.8-2.8L5.4 17.2 4 20z"/><path d="m14.2 7.4 2.8 2.8"/>',
  eye: '<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="2.8"/>',
  eyeoff:
    '<path d="m4 4 16 16"/><path d="M9.6 9.7a3 3 0 0 0 4.2 4.2"/><path d="M6.8 6.9C4 8.5 2.5 12 2.5 12s3.5 6.5 9.5 6.5c1.6 0 3-.4 4.2-1"/><path d="M9.9 5.7A9.6 9.6 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a17 17 0 0 1-2.2 3"/>',
  checkc: '<circle cx="12" cy="12" r="9"/><path d="m8.4 12 2.5 2.5 4.7-5"/>',
  alert: '<path d="M12 4 2.8 20h18.4z"/><path d="M12 10v4.2M12 17.4h.01"/>',
  xc: '<circle cx="12" cy="12" r="9"/><path d="m9.2 9.2 5.6 5.6M14.8 9.2l-5.6 5.6"/>',
  panelright: '<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M14.5 4v16"/>',
  panelleft: '<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M9.5 4v16"/>',
  key: '<circle cx="7.8" cy="15.6" r="3.4"/><path d="m10.2 13.2 8-8M15.6 5l2.6 2.6M13.6 7l2.2 2.2"/>',
  bot: '<path d="M12 8V4H8"/><rect width="16" height="12" x="4" y="8" rx="2"/><path d="M2 14h2M20 14h2M15 13v2M9 13v2"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7.2V12l3 1.8"/>',
  trash: '<path d="M4 7h16M9.5 7V4.5h5V7M6.5 7l.9 13h9.2l.9-13"/>',
  dollar:
    '<path d="M12 3v18"/><path d="M16.2 6.8C16.2 5.2 14.4 4 12 4S7.8 5.2 7.8 7s1.9 3 4.2 3 4.2 1.3 4.2 3-1.9 3-4.2 3-4.2-1.2-4.2-2.8"/>',
  chart:
    '<path d="M4.5 19.5V4.5"/><path d="M4.5 19.5h15"/><path d="M8 16.5v-4.5M12.3 16.5v-8M16.6 16.5v-5.5"/>',
  coins:
    '<ellipse cx="9" cy="6.5" rx="5.5" ry="2.6"/><path d="M3.5 6.5v4c0 1.4 2.5 2.6 5.5 2.6s5.5-1.2 5.5-2.6v-4"/><path d="M9 13c-3 0-5.5 1.2-5.5 2.6v.4c0 1.4 2.5 2.6 5.5 2.6 1 0 2-.1 2.8-.4"/><circle cx="16.5" cy="15.5" r="4.5"/>',
  book: '<path d="M4 5a2 2 0 0 1 2-2h12v15H6a2 2 0 0 0-2 2z"/><path d="M18 18v3H6a2 2 0 0 1-2-2"/>',
  layers: '<path d="m12 3 9 4.8-9 4.8L3 7.8z"/><path d="m3 12.5 9 4.8 9-4.8"/>',
  upright: '<path d="M8 16 16.5 7.5M9 7.2h8v8"/>',
  filter: '<path d="M3.5 5.5h17l-6.6 7.6v5l-3.8 1.9v-6.9z"/>',
  folderopen:
    '<path d="M3.5 7a2 2 0 0 1 2-2h2.8l2 2.2h6.2a2 2 0 0 1 2 2v.8H6.6L3.5 18z"/><path d="m3.5 18 2.9-7.8h16l-2.8 7.8z"/>',
  dot: '<circle cx="12" cy="12" r="3.4"/>',
  pause:
    '<rect x="6.5" y="5" width="3.5" height="14" rx="1"/><rect x="14" y="5" width="3.5" height="14" rx="1"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h.01"/>',
  branch2: '<path d="M6 4v16M6 8h7a3 3 0 0 0 3-3"/><circle cx="6" cy="4" r="1.6"/>',
  code: '<path d="m9 8-4 4 4 4M15 8l4 4-4 4"/>',
  flag: '<path d="M5 21V4M5 4h12l-2 4 2 4H5"/>',
  rocket:
    '<path d="M5 15c-1.5 1.5-2 5-2 5s3.5-.5 5-2a3 3 0 0 0-3-3z"/><path d="M8.5 13.5C8.5 9 12 4 19 4c0 7-5 10.5-9.5 10.5z"/><circle cx="14.5" cy="8.5" r="1.4"/>',
  puzzle:
    '<path d="M9.4 4.2a1.7 1.7 0 0 1 3.3 0c0 .7.5 1.1 1.2 1.1H16a1 1 0 0 1 1 1v2.1c0 .7.4 1.2 1.1 1.2a1.7 1.7 0 0 1 0 3.3c-.7 0-1.1.5-1.1 1.2V16a1 1 0 0 1-1 1h-2.1c-.7 0-1.2.5-1.2 1.2a1.7 1.7 0 0 1-3.3 0c0-.7-.5-1.2-1.2-1.2H5a1 1 0 0 1-1-1v-2.1c0-.7-.5-1.1-1.2-1.1a1.7 1.7 0 0 1 0-3.3c.7 0 1.2-.5 1.2-1.2V6.3a1 1 0 0 1 1-1h2.2c.7 0 1.2-.4 1.2-1.1z"/>',
  hook: '<path d="M18 6v7a5 5 0 0 1-10 0V9"/><circle cx="18" cy="4.5" r="1.6"/><path d="M8 9 5.5 6.5 8 4"/>',
  grid: '<rect x="3.5" y="3.5" width="7" height="7" rx="1.6"/><rect x="13.5" y="3.5" width="7" height="7" rx="1.6"/><rect x="3.5" y="13.5" width="7" height="7" rx="1.6"/><rect x="13.5" y="13.5" width="7" height="7" rx="1.6"/>',
  store:
    '<path d="M4 9h16l-1-4.5H5z"/><path d="M5 9v9.5a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V9"/><path d="M10 19.5V14h4v5.5"/>',
  split: '<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M3 12h18"/>',
  columns: '<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M12 4v16"/>',
  rows: '<path d="M4 7h16M4 12h16M4 17h16"/>',
  comment: '<path d="M20.5 11.5a7.5 7.5 0 0 1-10.6 6.8L4.5 19.5l1.2-4.3a7.5 7.5 0 1 1 14.8-3.7z"/>',
  archive:
    '<rect x="3" y="4" width="18" height="4.5" rx="1.4"/><path d="M5 8.5V19a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8.5"/><path d="M10 12.5h4"/>',
  maximize:
    '<path d="M9 4H5.5a1.5 1.5 0 0 0-1.5 1.5V9"/><path d="M15 4h3.5A1.5 1.5 0 0 1 20 5.5V9"/><path d="M9 20H5.5A1.5 1.5 0 0 1 4 18.5V15"/><path d="M15 20h3.5a1.5 1.5 0 0 0 1.5-1.5V15"/>',
  minimize:
    '<path d="M4.5 9H9V4.5"/><path d="M19.5 9H15V4.5"/><path d="M4.5 15H9v4.5"/><path d="M19.5 15H15v4.5"/>',
  crosshair:
    '<circle cx="12" cy="12" r="7.5"/><path d="M12 2.5v3.5M12 18v3.5M2.5 12H6M18 12h3.5"/>',
  users:
    '<circle cx="9.5" cy="8" r="3.2"/><path d="M3.5 19.5c0-3.1 2.7-5.2 6-5.2s6 2.1 6 5.2"/><path d="M16.5 5.2a3.2 3.2 0 0 1 0 5.9"/><path d="M18 14.6c1.7.8 2.8 2.3 2.8 4.4"/>',
  compress: '<path d="M4 6h16M4 18h16"/><path d="m9 10 3-2.5 3 2.5"/><path d="m9 14 3 2.5 3-2.5"/>',
  lightbulb: '<path d="M9.2 16.5a5.6 5.6 0 1 1 5.6 0v1.7H9.2z"/><path d="M10 21h4"/>',
  squarepen:
    '<path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.4 2.6a2.1 2.1 0 0 1 3 3L12 15l-4 1 1-4Z"/>',
  listtodo:
    '<rect x="3" y="4" width="4" height="4" rx="1"/><path d="m4.5 6 1 1 2-2"/><path d="M10 6h11"/><rect x="3" y="16" width="4" height="4" rx="1"/><path d="m4.5 18 1 1 2-2"/><path d="M10 18h11"/><path d="M4 12h3M10 12h11"/>',
  inbox:
    '<polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11Z"/>',
  chartcolumn: '<path d="M3 3v16a2 2 0 0 0 2 2h16"/><path d="M18 17V9M13 17V5M8 17v-3"/>',
  settings2:
    '<path d="M20 7h-9M14 17H5"/><circle cx="17" cy="17" r="3"/><circle cx="7" cy="7" r="3"/>',
  blocks:
    '<rect x="3" y="3" width="7" height="7" rx="2"/><rect x="14" y="3" width="7" height="7" rx="2"/><rect x="3" y="14" width="7" height="7" rx="2"/><path d="M17.5 14v7M14 17.5h7"/>',
  chartup: '<path d="M4 20V10M4 20h16"/><path d="m7 15 4-4 3 3 6-7"/><path d="M16 7h4v4"/>',
  layouttemplate: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 21V9"/>',
  filetext:
    '<path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6M8 13h8M8 17h6"/>',
  packagecheck:
    '<path d="m12 3 8 4.5v9L12 21l-8-4.5v-9z"/><path d="m4.5 7.8 7.5 4.3 7.5-4.3M12 12.1V21"/><path d="m8.5 9.9 7.7-4.4"/><path d="m14.5 15 1.5 1.5 3-3"/>',
  database:
    '<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v6c0 1.7 3.6 3 8 3s8-1.3 8-3V5"/><path d="M4 11v6c0 1.7 3.6 3 8 3s8-1.3 8-3v-6"/>',
  library: '<path d="M4 19.5V5a2 2 0 0 1 2-2h12v16H6a2 2 0 0 0-2 2z"/><path d="M8 7h6M8 11h7"/>',
  gauge: '<path d="m12 14 4-4"/><path d="M3.34 19a10 10 0 1 1 17.32 0"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/>',
  moon: '<path d="M21 12.8A8.5 8.5 0 1 1 11.2 3a6.5 6.5 0 0 0 9.8 9.8z"/>',
  monitor: '<rect x="2.5" y="3.5" width="19" height="13" rx="2"/><path d="M8 21h8M12 17v4"/>',
};

/** 旧调用点名 → 原型注册表键（历史命名兼容，避免全库改调用点）。 */
const ICON_ALIASES: Record<string, string> = {
  slidersh: 'slidershorizontal',
  expand: 'maximize',
  restore: 'copy',
};

export type IconName = keyof typeof ICON_PATHS | keyof typeof ICON_ALIASES;

const TAG_MATCH = /<(path|rect|circle|polyline|ellipse)\b([^>]*?)\/>/g;
const ATTR_MATCH = /([a-zA-Z-]+)="([^"]*)"/g;

/** 把原型路径串解析成 React 元素（原型条目只含这五种自闭合图形）。 */
function parseIconShapes(markup: string): ReactElement[] {
  const elements: ReactElement[] = [];
  for (const match of markup.matchAll(TAG_MATCH)) {
    const tag = match[1];
    const attrs: Record<string, string | number> = {};
    for (const attr of match[2].matchAll(ATTR_MATCH)) {
      attrs[attr[1]] = attr[2];
    }
    elements.push(createElement(tag, { key: elements.length, ...attrs }));
  }
  return elements;
}

export function Icon({
  name,
  className,
  style,
}: {
  name: IconName;
  className?: string;
  style?: CSSProperties;
}) {
  const key = ICON_ALIASES[name] ?? name;
  const markup = ICON_PATHS[key] ?? ICON_PATHS.dot;
  return (
    <svg
      className={className}
      style={style}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {parseIconShapes(markup)}
    </svg>
  );
}
