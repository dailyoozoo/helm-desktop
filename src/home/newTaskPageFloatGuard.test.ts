import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

/**
 * 新任务页浮层根因守卫（2026-08-26 随组件库收敛改版）。
 *
 * 历史：`.home-composer:focus-within { transform: translateY(-1px) }` 曾把
 * composer 变成 fixed 浮层的包含块与独立 stacking context——模式/权限浮层整体
 * 右移、模型/强度浮层移出视口、加号菜单被 z-25 遮罩拦截。R6 整改后浮层经
 * HomeFloat portal 挂 document.body，composer 不再决定浮层坐标，但两条不变量
 * 必须继续锁死；composer 自身则永久禁止 transform（现役口径②，
 * docs/已知限制.md「新任务页像素对齐」）。
 */
const appCss = readFileSync(new URL('../../src/styles/app.css', import.meta.url), 'utf8');
const componentsCss = readFileSync(
  new URL('../../src/styles/components.css', import.meta.url),
  'utf8',
);
const newTaskPage = readFileSync(new URL('./NewTaskPage.tsx', import.meta.url), 'utf8');

function ruleBody(selector: string): string {
  const start = appCss.indexOf(selector + ' {');
  expect(start, 'app.css 应包含规则 ' + selector).toBeGreaterThanOrEqual(0);
  const open = appCss.indexOf('{', start);
  const close = appCss.indexOf('}', open);
  return appCss.slice(open + 1, close);
}

/** 在任意 CSS 文本中提取第一条 `selector { … }` 规则体（供多文件断言复用）。 */
function ruleBodyFrom(css: string, selector: string): string {
  const start = css.indexOf(selector + ' {');
  expect(start, 'CSS 应包含规则 ' + selector).toBeGreaterThanOrEqual(0);
  const open = css.indexOf('{', start);
  const close = css.indexOf('}', open);
  return css.slice(open + 1, close);
}

describe('新任务页浮层根因守卫', () => {
  it('fixed 浮层变体保持 body 级 z-index 320（原型 .floatmenu 同值）', () => {
    const body = ruleBody('.home-floatmenu--fixed');
    expect(body).toContain('position: fixed');
    expect(body).toMatch(/z-index:\s*320/);
  });

  it('HomeFloat 经 createPortal 渲染到 document.body（对齐原型 openMenuFloat）', () => {
    expect(newTaskPage).toContain('createPortal(');
    expect(newTaskPage).toContain('document.body,');
  });

  it('composer 聚焦禁止 transform：本页覆盖必须显式压掉共享层 translateY(-1px)', () => {
    // 共享层 components.css 的 .cm-composer:focus-within 带 transform（原型后层样式）；
    // 本页以更高特异性的覆盖禁用之（app.css「现役口径②」）。若有人删除本覆盖，
    // composer 将重新成为包含块，历史缺陷回归。
    const body = ruleBody('.home--start .cm-composer:focus-within');
    expect(body.replace(/\s/g, '')).toContain('transform:none');
  });

  it('加号菜单 z-index 必须压过页内遮罩（遮罩 z25，菜单须 ≥30）', () => {
    const overlay = ruleBody('.home-overlay');
    expect(overlay).toMatch(/z-index:\s*25/);
    const menu = ruleBody('.home--start .cm-menu');
    const menuZ = Number(menu.match(/z-index:\s*(\d+)/)?.[1]);
    const overlayZ = Number(overlay.match(/z-index:\s*(\d+)/)?.[1]);
    expect(menuZ).toBeGreaterThan(overlayZ);
  });

  it('共享层 .cm-start 禁止 flex 拉伸：垂直居中只由 .home--start .cm-start 的 margin:auto 承担', () => {
    // 2026-09-03 用户报告「描述行太高」根因：组件库收敛时把原型编辑层的
    // flex column + flex:1 误植进共享层 .cm-start，起始块被拉伸满高，
    // 标题钉在页顶、composer 钉在页底（现役口径①失效）。
    const shared = ruleBodyFrom(componentsCss, '.cm-start');
    const compact = shared.replace(/\s/g, '');
    expect(compact).not.toContain('flex:1');
    expect(compact).not.toContain('flex-direction:column');
    // 本页覆盖仍存在（现役口径①：margin auto 安全垂直居中）。
    const scoped = ruleBody('.home--start .cm-start');
    expect(scoped.replace(/\s/g, '')).toContain('margin:auto');
  });

  it('共享层 .cm-compose-shell 禁止 margin-top:auto（启动序列隐藏后会把 composer 钉在页底）', () => {
    const body = ruleBodyFrom(componentsCss, '.cm-compose-shell');
    expect(body.replace(/\s/g, '')).not.toContain('margin-top:auto');
  });
});
