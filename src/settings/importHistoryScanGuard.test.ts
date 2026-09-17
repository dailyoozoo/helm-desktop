import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

/**
 * 导入历史对话弹窗「引擎切换串台」守卫（2026-09-08 用户反馈修复）。
 *
 * 缺陷：先点 Claude 看到列表 → 返回 → 再点 Codex，面包屑已是 Codex，
 * 列表却显示上一次 Claude 的扫描结果。根因：扫描 effect 只重置
 * loading/error，没有清 scan——Codex 扫描要全量读几百个 JSONL 耗时数秒，
 * 这段时间旧列表原样挂在页面上；若扫描请求失败旧列表还会永久残留。
 *
 * 不变量：进入「选择对话」步骤的扫描 effect 必须先 setScan(null) 再发起
 * 扫描。渲染级断言依赖 jsdom，当前测试环境只有 node，故以源码守卫锁死
 * （与 newTaskPageDirModalGuard 同手法）。
 */
const source = readFileSync(new URL('./AboutTab.tsx', import.meta.url), 'utf8');

/** 取「选择对话」扫描 effect 的函数体（从标记行到依赖数组收尾）。 */
function scanEffectBody(): string {
  const marker = "if (step !== 'select-conv') return;";
  const start = source.indexOf(marker);
  expect(start, 'AboutTab.tsx 应包含扫描 effect').toBeGreaterThanOrEqual(0);
  const end = source.indexOf('}, [engine, step]);', start);
  expect(end, '扫描 effect 应以 [engine, step] 为依赖').toBeGreaterThan(start);
  return source.slice(start, end);
}

describe('导入历史对话弹窗引擎切换守卫', () => {
  it('切换引擎重新扫描前必须清空上一次的 scan 结果', () => {
    expect(scanEffectBody()).toContain('setScan(null)');
  });

  it('扫描期间必须有 loading 与 error 状态重置，避免旧提示残留', () => {
    const body = scanEffectBody();
    expect(body).toContain('setLoading(true)');
    expect(body).toContain('setError(null)');
  });

  it('点击引擎进入二级页时同步清空勾选与导入结果', () => {
    // 两个引擎按钮的 onClick 都要先 setSelected([]) + setResults([])
    const buttonCount = source.split('setSelected([])').length - 1;
    const resultsCount = source.split('setResults([])').length - 1;
    expect(buttonCount).toBeGreaterThanOrEqual(2);
    expect(resultsCount).toBeGreaterThanOrEqual(2);
  });
});
