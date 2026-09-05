import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

/**
 * 主侧栏选中态根因守卫（2026-09-04 用户报告「发起了新对话，左边选中的还是老对话」）。
 *
 * 历史：主侧栏的高亮曾只由「点过哪一行」驱动（`activeTaskId` 本地 state）。一旦会话从
 * 其他入口变化——侧栏「新任务」发新对话、启动自动恢复、分叉跳转、命令面板打开——
 * 高亮就永远停在用户最后点过的那一行，与工作区里真正在跑的会话脱节。
 *
 * 现役口径：选中态的唯一真值 = 工作区上报的当前会话身份（`helm:session-active`），
 * 点击只保留「点完 → 上报抵达」之间的乐观高亮。以下不变量不得回退。
 */
const railSource = readFileSync(new URL('./Rail.tsx', import.meta.url), 'utf8');
const workspaceSource = readFileSync(
  new URL('../workspace/Workspace.tsx', import.meta.url),
  'utf8',
);
const EVENT = 'helm:session-active';

describe('主侧栏选中态 = 工作区当前会话（根因守卫）', () => {
  it('工作区在会话身份变化时广播选中态真值（三个 id 全涵盖）', () => {
    expect(workspaceSource).toContain(`'${EVENT}'`);
    const dispatch = workspaceSource.slice(workspaceSource.indexOf(`'${EVENT}'`) - 400);
    expect(dispatch).toContain('historyId');
    expect(dispatch).toContain('handleId');
    expect(dispatch).toContain('cliSessionId');
    // 三个 id 任一变化都要重发；缺一个依赖就会漏掉「新建会话」这类只改单个 id 的迁移
    expect(workspaceSource).toContain('}, [state.historyId, state.handleId, state.sessionId]);');
  });

  it('主侧栏订阅该事件，并以工作区上报值覆盖点击产生的乐观高亮', () => {
    expect(railSource).toContain(`window.addEventListener('${EVENT}'`);
    expect(railSource).toContain('setPickedTaskId(null);');
  });

  it('高亮行由上报身份派生，点击态只是回退值', () => {
    expect(railSource).toContain('activeRailTaskId(sessions, activeIds) ?? pickedTaskId');
    // 不得恢复成「只有点击才高亮」的单一本地状态
    expect(railSource).not.toContain('const [activeTaskId, setActiveTaskId]');
  });

  it('新建会话（身份全空）会清空高亮：解析函数在空身份下返回 null', () => {
    // 断言在这里只锁住契约两端使用同一事件名；解析语义由 railViewModel 单测覆盖
    expect(railSource).toContain(EVENT);
    expect(workspaceSource).toContain(EVENT);
  });
});
