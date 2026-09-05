import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { Composer, settleQueuedMessage, attachmentPills, effortDescription } from './Composer';

describe('Composer', () => {
  it('attachmentPills：路径转成 attachment 药丸并取文件名', () => {
    const pills = attachmentPills(['D:/work/log.txt', 'D:/work/src/main.ts'], 'D:/work');
    expect(pills).toEqual([
      { kind: 'attachment', path: 'D:/work/log.txt', label: 'log.txt' },
      { kind: 'attachment', path: 'D:/work/src/main.ts', label: 'main.ts' },
    ]);
  });

  it('自动发送失败时保留消息和附件，成功后才移出队列', () => {
    const first = { text: '继续修复', attachments: ['D:/work/log.txt'] };
    const second = { text: '再跑测试', attachments: [] };
    const queue = [first, second];

    expect(settleQueuedMessage(queue, first, false)).toEqual({ queue, paused: true });
    expect(settleQueuedMessage(queue, first, true)).toEqual({ queue: [second], paused: false });
  });

  it('输入区只保留上下文与权限说明，不重复头部的模型和累计花费', () => {
    const markup = renderToStaticMarkup(
      <Composer
        working={false}
        mode="build"
        engine="codex"
        reasoningEffort="high"
        permissionProfile="standard"
        onModeChange={() => {}}
        onCommandAction={() => {}}
        onSend={async () => true}
        onStop={() => {}}
        cost={{
          inputTokens: 100,
          outputTokens: 20,
          contextTokens: 80,
          contextWindow: 100,
          costUsd: 0.1234,
        }}
      />,
    );

    expect(markup).not.toContain('gpt-5-codex');
    expect(markup).toContain('上下文 80%');
    expect(markup).not.toContain('$0.1234');
    // 批次③：权限档位成为底栏 cm-tool（原型 #profBtn），当前档位文案在按钮上
    expect(markup).toContain('标准');
  });

  it('批次③：底栏含模式/权限/模型/强度入口，快捷键提示行撤除', () => {
    const markup = renderToStaticMarkup(
      <Composer
        working={false}
        mode="build"
        engine="codex"
        reasoningEffort="high"
        permissionProfile="standard"
        onModeChange={() => {}}
        onCommandAction={() => {}}
        onSend={async () => true}
        onStop={() => {}}
        cost={{
          inputTokens: 100,
          outputTokens: 20,
          contextTokens: 80,
          contextWindow: 100,
          costUsd: 0.1234,
        }}
      />,
    );
    // 不再渲染独立的斜杠入口按钮（输入 / 已可发现）
    expect(markup).not.toContain('title="斜杠命令');
    // 底栏模式/权限/模型/强度（原型 #modeBtn/#profBtn/#modelBtn/#effortBtn）
    expect(markup).toContain('任务模式：构建可写文件执行命令');
    expect(markup).toContain('权限档位');
    expect(markup).toContain('下一轮模型偏好');
    expect(markup).toContain('推理强度：独立于模型');
    // 发送按钮保持 aria-label
    expect(markup).toContain('aria-label="发送"');
    // 批次③：快捷键提示行撤除（原型无此行）
    expect(markup).not.toContain('Shift+Enter');
  });

  it('本轮修复：输入框提示语与权限按钮用全称（原型 L1085/applyProf）', () => {
    const base = {
      working: false,
      mode: 'build' as const,
      engine: 'claude-code',
      onModeChange: () => {},
      onCommandAction: () => {},
      onSend: async () => true,
      onStop: () => {},
    };
    const build = renderToStaticMarkup(<Composer {...base} permissionProfile="standard" />);
    expect(build).toContain('Enter 发送 · / 唤起命令');
    expect(build).not.toContain('@ 文件');

    const auto = renderToStaticMarkup(<Composer {...base} permissionProfile="auto" />);
    expect(auto).toContain('自动执行');

    const fullAccess = renderToStaticMarkup(<Composer {...base} permissionProfile="full_access" />);
    expect(fullAccess).toContain('全部放开');
  });

  it('本轮修复：「+」能力菜单只保留原型 3 项（无文件夹直选项）', () => {
    const markup = renderToStaticMarkup(
      <Composer
        working={false}
        mode="build"
        engine="claude-code"
        permissionProfile="standard"
        onModeChange={() => {}}
        onCommandAction={() => {}}
        onSend={async () => true}
        onStop={() => {}}
      />,
    );
    expect(markup).toContain('文件与目录');
    expect(markup).toContain('命令与技能');
    expect(markup).toContain('连接器');
    expect(markup).not.toContain('文件夹加入上下文');
  });

  it('本轮修复：强度档位说明按引擎取原型文案', () => {
    // 浮层菜单仅 openMenu 时渲染；静态渲染下直接验证档位文案映射（原型 EFFORT_DESC）
    expect(effortDescription('low', 'claude-code')).toBe('更快返回，适合简单修改与明确指令。');
    expect(effortDescription('high', 'claude-code')).toBe('增加分析预算，适合复杂重构和排障。');
    expect(effortDescription('high', 'codex')).toBe('增加分析预算，适合复杂编码任务。');
    expect(effortDescription('auto')).toBe('使用当前模型的默认推理预算。');
    expect(effortDescription('max')).toBe('使用当前引擎支持的最大推理预算。');
  });

  it('切片 D · P2-01：运行时只出停止按钮，不再渲染发送按钮', () => {
    const markup = renderToStaticMarkup(
      <Composer
        working
        mode="build"
        engine="codex"
        permissionProfile="standard"
        onModeChange={() => {}}
        onCommandAction={() => {}}
        onSend={async () => true}
        onStop={() => {}}
        cost={{
          inputTokens: 100,
          outputTokens: 20,
          contextTokens: 80,
          contextWindow: 100,
          costUsd: 0.1234,
        }}
      />,
    );
    // 运行中：停止按钮在，发送按钮整体不渲染（Enter 走排队路径）
    expect(markup).toContain('停止');
    expect(markup).not.toContain('cm-tool--send');
    expect(markup).not.toContain('aria-label="发送"');
    // 排队能力对用户可见：输入框提示与无障碍标签都说明 Enter 会排队
    expect(markup).toContain('Enter 加入队列');
    expect(markup).toContain('消息输入框（运行中，Enter 排队）');
  });

  it('B 方案无感恢复：holding（引擎恢复中）仍显示发送按钮，可打字可点发送（走排队而非顶掉输入框）', () => {
    const markup = renderToStaticMarkup(
      <Composer
        working={false}
        holding
        mode="build"
        engine="codex"
        permissionProfile="standard"
        onModeChange={() => {}}
        onCommandAction={() => {}}
        onSend={async () => true}
        onStop={() => {}}
      />,
    );
    // holding 态：输入框在场、发送按钮在场（不是「正在恢复…」横幅），
    // 消息点击后进队列、句柄就绪自动 flush——用户无感。
    expect(markup).toContain('cm-tool--send');
    expect(markup).toContain('aria-label="发送"');
    // 未拿到句柄时不能变成「停止当前轮次」（那会谎报有轮次在跑）
    expect(markup).not.toContain('aria-label="停止当前轮次"');
  });

  it('切片 D · P2-01：空闲时渲染发送按钮且不出现停止按钮', () => {
    const markup = renderToStaticMarkup(
      <Composer
        working={false}
        mode="build"
        engine="codex"
        permissionProfile="standard"
        onModeChange={() => {}}
        onCommandAction={() => {}}
        onSend={async () => true}
        onStop={() => {}}
      />,
    );
    expect(markup).toContain('cm-tool--send');
    expect(markup).toContain('aria-label="发送"');
    expect(markup).not.toContain('aria-label="停止当前轮次"');
  });

  it('全部放开确认卡默认不渲染，与新任务页共用 wsconfirm 文案组件', () => {
    const markup = renderToStaticMarkup(
      <Composer
        working={false}
        mode="build"
        engine="codex"
        permissionProfile="standard"
        onModeChange={() => {}}
        onCommandAction={() => {}}
        onSend={async () => true}
        onStop={() => {}}
      />,
    );
    expect(markup).not.toContain('开启「全部放开」？');
    expect(markup).not.toContain('开启全部放开');
  });
});
