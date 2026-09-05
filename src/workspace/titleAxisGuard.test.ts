import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

/**
 * 任务标题与 composer 左对齐根因守卫（2026-09-04 用户报告「标题跟输入框不对齐了」）。
 *
 * 历史：对齐靠一次性的挂载期观测（`useEffect(..., [])`）把 `.composer__inner` 的左缘
 * 换算成 `.titlebar__center` 的 padding-left。09-04 起恢复窗口会整块卸载 Composer
 * （`resumingPlaceholder`），那次挂载要么压根取不到 `.composer__inner` 直接 return，
 * 要么盯在一个随后被卸载的节点上——恢复完成后 Composer 重新挂载也再无同步，
 * 标题就永久贴在标题栏最左边。
 *
 * 现役口径：随 Composer 在场与否重建观测，并同时观测父级 `.composer`
 * （右栏开合 / 轮次轨道出现只改内边距，`.composer__inner` 自身宽度可能不变，只盯它会漏触发）。
 */
const workspaceSource = readFileSync(new URL('./Workspace.tsx', import.meta.url), 'utf8');

/** 抽出 syncTitleAxis 那段 effect（从 .titlebar__center 查找到依赖数组结束）。 */
function titleAxisEffect(): string {
  const start = workspaceSource.indexOf(".titlebar__center'");
  expect(start, 'Workspace 应保留 syncTitleAxis 的标题栏对齐逻辑').toBeGreaterThan(-1);
  const end = workspaceSource.indexOf('}, [', start);
  expect(end, '应当存在依赖数组').toBeGreaterThan(start);
  return workspaceSource.slice(start, workspaceSource.indexOf(']);', end) + 3);
}

describe('任务标题左缘与 composer 对齐（根因守卫）', () => {
  it('依赖数组必须带上 Composer 在场与否：恢复窗口卸载 Composer 后要能重新同步', () => {
    const effect = titleAxisEffect();
    expect(effect).toContain('resumingPlaceholder');
  });

  it('除 .composer__inner 外还要观测父级 .composer，避免只改内边距时漏触发', () => {
    const effect = titleAxisEffect();
    expect(effect).toContain("querySelector<HTMLElement>('.composer__inner')");
    expect(effect).toContain('inner.parentElement');
    expect(effect).toContain('ro.observe(inner.parentElement)');
  });

  it('卸载时复位 padding-left，避免非工作区页沿用上一次的缩进', () => {
    const effect = titleAxisEffect();
    expect(effect).toContain("center.style.paddingLeft = ''");
    expect(effect).toContain('ro.disconnect()');
  });
});
