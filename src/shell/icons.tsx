import type { CSSProperties } from 'react';

// 图标集 —— 与 prototype/assets/app.js 的 P 表一致（stroke=currentColor，由父级 CSS 控制尺寸）。
const PATHS = {
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
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.4-3.4"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  send: '<path d="M12 19V5M6 11l6-6 6 6"/>',
  clip: '<path d="M20.5 11.5 12 20a4.6 4.6 0 0 1-6.5-6.5l8.4-8.4a3 3 0 0 1 4.3 4.3l-8.4 8.4a1.4 1.4 0 0 1-2-2l7.7-7.7"/>',
  down: '<path d="m6 9 6 6 6-6"/>',
  right: '<path d="m9 6 6 6-6 6"/>',
  left: '<path d="m15 6-6 6 6 6"/>',
  up: '<path d="m6 15 6-6 6 6"/>',
  check: '<path d="m5 12 5 5L20 6"/>',
  x: '<path d="M6 6 18 18M18 6 6 18"/>',
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
  plug: '<path d="M9 3v5.5M15 3v5.5M7 8.5h10v2.2a5 5 0 0 1-10 0z"/><path d="M12 15.7V21"/>',
  sliders:
    '<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2.2"/><circle cx="9" cy="17" r="2.2"/>',
  palette:
    '<path d="M12 3a9 9 0 1 0 0 18c1.3 0 1.8-1 1.8-1.9 0-1.4 1-1.9 2.1-1.9H17a4 4 0 0 0 4-4C21 7.3 17 3 12 3z"/><circle cx="7.5" cy="11" r="1"/><circle cx="12" cy="7.8" r="1"/><circle cx="16.3" cy="11" r="1"/>',
  keyboard:
    '<rect x="3" y="6" width="18" height="12" rx="2.5"/><path d="M7 10h.01M11 10h.01M15 10h.01M17 10h.01M8.5 14h7"/>',
  more: '<circle cx="5" cy="12" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="19" cy="12" r="1.5"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2.5"/><path d="M5 15a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2"/>',
  play: '<path d="M7 5 19 12 7 19z"/>',
  stop: '<rect x="6.5" y="6.5" width="11" height="11" rx="2.5"/>',
  refresh:
    '<path d="M20 11a8 8 0 0 0-13.8-5L4 8"/><path d="M4 3.5V8h4.5"/><path d="M4 13a8 8 0 0 0 13.8 5L20 16"/><path d="M20 20.5V16h-4.5"/>',
  edit: '<path d="M4 20h4.2L19 9.2a2 2 0 0 0-2.8-2.8L5.4 17.2 4 20z"/><path d="m14.2 7.4 2.8 2.8"/>',
  checkc: '<circle cx="12" cy="12" r="9"/><path d="m8.4 12 2.5 2.5 4.7-5"/>',
  alert: '<path d="M12 4 2.8 20h18.4z"/><path d="M12 10v4.2M12 17.4h.01"/>',
  xc: '<circle cx="12" cy="12" r="9"/><path d="m9.2 9.2 5.6 5.6M14.8 9.2l-5.6 5.6"/>',
  panelright: '<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M14.5 4v16"/>',
  key: '<circle cx="7.8" cy="15.6" r="3.4"/><path d="m10.2 13.2 8-8M15.6 5l2.6 2.6M13.6 7l2.2 2.2"/>',
  eye: '<path d="M2.5 12s3.5-6 9.5-6 9.5 6 9.5 6-3.5 6-9.5 6-9.5-6-9.5-6z"/><circle cx="12" cy="12" r="2.8"/>',
  eyeoff:
    '<path d="m3 3 18 18"/><path d="M10.6 6.2A10.8 10.8 0 0 1 12 6c6 0 9.5 6 9.5 6a16 16 0 0 1-3 3.6"/><path d="M6.7 6.8C4 8.5 2.5 12 2.5 12s3.5 6 9.5 6c1.3 0 2.5-.3 3.5-.7"/><path d="M10.3 10.3a2.8 2.8 0 0 0 3.4 3.4"/>',
  bot: '<rect x="4" y="8" width="16" height="11" rx="3.2"/><path d="M12 8V4.5M9 19v2M15 19v2"/><circle cx="12" cy="4" r="1"/><path d="M9 13h.01M15 13h.01"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7.2V12l3 1.8"/>',
  dollar:
    '<path d="M12 3v18"/><path d="M16.2 6.8C16.2 5.2 14.4 4 12 4S7.8 5.2 7.8 7s1.9 3 4.2 3 4.2 1.3 4.2 3-1.9 3-4.2 3-4.2-1.2-4.2-2.8"/>',
  chart:
    '<path d="M4.5 19.5V4.5"/><path d="M4.5 19.5h15"/><path d="M8 16.5v-4.5M12.3 16.5v-8M16.6 16.5v-5.5"/>',
  book: '<path d="M4 5a2 2 0 0 1 2-2h12v15H6a2 2 0 0 0-2 2z"/><path d="M18 18v3H6a2 2 0 0 1-2-2"/>',
  layers: '<path d="m12 3 9 4.8-9 4.8L3 7.8z"/><path d="m3 12.5 9 4.8 9-4.8"/>',
  upright: '<path d="M8 16 16.5 7.5M9 7.2h8v8"/>',
  folderopen:
    '<path d="M3.5 7a2 2 0 0 1 2-2h2.8l2 2.2h6.2a2 2 0 0 1 2 2v.8H6.6L3.5 18z"/><path d="m3.5 18 2.9-7.8h16l-2.8 7.8z"/>',
  flag: '<path d="M5 21V4M5 4h12l-2 4 2 4H5"/>',
  rocket:
    '<path d="M5 15c-1.5 1.5-2 5-2 5s3.5-.5 5-2a3 3 0 0 0-3-3z"/><path d="M8.5 13.5C8.5 9 12 4 19 4c0 7-5 10.5-9.5 10.5z"/><circle cx="14.5" cy="8.5" r="1.4"/>',
  puzzle:
    '<path d="M9.4 4.2a1.7 1.7 0 0 1 3.3 0c0 .7.5 1.1 1.2 1.1H16a1 1 0 0 1 1 1v2.1c0 .7.4 1.2 1.1 1.2a1.7 1.7 0 0 1 0 3.3c-.7 0-1.1.5-1.1 1.2V16a1 1 0 0 1-1 1h-2.1c-.7 0-1.2.5-1.2 1.2a1.7 1.7 0 0 1-3.3 0c0-.7-.5-1.2-1.2-1.2H5a1 1 0 0 1-1-1v-2.1c0-.7-.5-1.1-1.2-1.1a1.7 1.7 0 0 1 0-3.3c.7 0 1.2-.5 1.2-1.2V6.3a1 1 0 0 1 1-1h2.2c.7 0 1.2-.4 1.2-1.1z"/>',
  code: '<path d="m9 8-4 4 4 4M15 8l4 4-4 4"/>',
  store:
    '<path d="M3 9h18M3 9 5 4h14l2 5M3 9v10a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V9"/><path d="M8.5 9a2 2 0 0 1-2 2 2 2 0 0 1-2-2M15.5 9a2 2 0 0 1-2 2 2 2 0 0 1-2-2M8.5 9a2 2 0 0 0 2 2 2 2 0 0 0 2-2M15.5 9a2 2 0 0 0 2 2 2 2 0 0 0 2-2"/>',
} as const;

export type IconName = keyof typeof PATHS;

export function Icon({
  name,
  className,
  style,
}: {
  name: IconName;
  className?: string;
  style?: CSSProperties;
}) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      style={style}
      dangerouslySetInnerHTML={{ __html: PATHS[name] }}
    />
  );
}
