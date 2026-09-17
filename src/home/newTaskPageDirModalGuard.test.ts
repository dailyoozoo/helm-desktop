import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

/**
 * 新任务页「选择工作目录」弹层关闭守卫（2026-09-08 用户反馈修复）。
 *
 * 缺陷：就绪检查弹层里点「目录」会叠开工作目录弹层；其中「从电脑选择…」走
 * handlePickDirectory，历史实现只 setDirectory 不关本弹层——选完目录后弹层仍
 * 盖在就绪检查之上，表现为「工作目录弹框没消失」。
 *
 * 不变量：目录弹层的三条出口（最近目录行 / 从电脑选择 / 跳页）都必须显式
 * setDirModalOpen(false)。渲染级断言依赖 jsdom，当前测试环境只有 node，
 * 故以源码守卫锁死（与 newTaskPageFloatGuard 同手法）。
 */
const source = readFileSync(new URL('./NewTaskPage.tsx', import.meta.url), 'utf8');

/** 从 `const <name> = ... => {` 起做花括号配平，取出函数体。 */
function bodyOf(name: string): string {
  const marker = 'const ' + name + ' =';
  const start = source.indexOf(marker);
  expect(start, 'NewTaskPage.tsx 应定义 ' + name).toBeGreaterThanOrEqual(0);
  const open = source.indexOf('{', start);
  let depth = 0;
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === '{') depth += 1;
    else if (source[i] === '}') {
      depth -= 1;
      if (depth === 0) return source.slice(open + 1, i);
    }
  }
  throw new Error('未配平的函数体：' + name);
}

describe('新任务页目录弹层关闭守卫', () => {
  it('「从电脑选择…」：选中目录后必须关闭目录弹层', () => {
    const body = bodyOf('handlePickDirectory');
    expect(body).toContain('setDirModalOpen(false)');
  });

  it('「从电脑选择…」：与最近目录行一致，选完即用真实报告复检', () => {
    expect(bodyOf('handlePickDirectory')).toContain('refreshReadiness()');
    expect(bodyOf('chooseDirectoryPath')).toContain('refreshReadiness()');
  });

  it('最近目录行：选中即关弹层', () => {
    expect(bodyOf('chooseDirectoryPath')).toContain('setDirModalOpen(false)');
  });

  it('跳页（去配置服务商等）：目录弹层与中心弹层一并收起，避免遮罩残留', () => {
    const body = bodyOf('navigateAway');
    expect(body).toContain('setDirModalOpen(false)');
    expect(body).toContain('setCenter(null)');
  });

  it('系统目录选择器期间加锁：目录弹层入口按钮禁用重复点击', () => {
    const pick = source.indexOf('data-home-pick-dir');
    expect(pick).toBeGreaterThan(0);
    // 就近断言：该按钮标签（从 <button 到闭合 >）内必须出现 disabled={fileDialogBusy}
    const tagStart = source.lastIndexOf('<button', pick);
    const tagEnd = source.indexOf('>', pick);
    expect(source.slice(tagStart, tagEnd)).toContain('disabled={fileDialogBusy}');
  });
});
