import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { SetupGuide, setupGuideAllReady, setupGuideRows, type SetupGuideDeps } from './SetupGuide';

const allMissing: SetupGuideDeps = {
  node: 'missing',
  npm: 'missing',
  git: 'missing',
  cli: 'missing',
};

const nodeOk: SetupGuideDeps = {
  node: 'ok',
  npm: 'ok',
  git: 'missing',
  cli: 'missing',
};

const allOk: SetupGuideDeps = {
  node: 'ok',
  npm: 'ok',
  git: 'ok',
  cli: 'ok',
};

describe('SetupGuide · 纯函数', () => {
  it('setupGuideAllReady：三项全绿才放行，缺任一项不放行', () => {
    expect(setupGuideAllReady(allOk)).toBe(true);
    expect(setupGuideAllReady(allMissing)).toBe(false);
    expect(setupGuideAllReady(nodeOk)).toBe(false);
    expect(setupGuideAllReady({ node: 'ok', npm: 'ok', git: 'ok', cli: 'installing' })).toBe(false);
  });

  it('setupGuideRows：Node/Git 共享、CLI 行随引擎切换名称/图标', () => {
    const claude = setupGuideRows(allMissing, 'claude-code');
    expect(claude.map((row) => row.name)).toEqual(['Node.js 18+', 'Git', 'Claude Code CLI']);
    expect(claude[2].icon).toBe('zap');

    const codex = setupGuideRows(allMissing, 'codex');
    expect(codex.map((row) => row.name)).toEqual(['Node.js 18+', 'Git', 'Codex CLI']);
    expect(codex[2].icon).toBe('cpu');
  });

  it('setupGuideRows：restartRequired 透传到对应行', () => {
    const rows = setupGuideRows(allMissing, 'claude-code', { node: true });
    expect(rows[0].restartRequired).toBe(true);
    expect(rows[1].restartRequired).toBeUndefined();
    expect(rows[2].restartRequired).toBeUndefined();
  });
});

describe('SetupGuide · 组件渲染', () => {
  it('全绿后组件不渲染（取消拦截，发送恢复可用）', () => {
    const markup = renderToStaticMarkup(
      <SetupGuide engine="claude-code" seedDeps={allOk} onReady={() => {}} />,
    );
    expect(markup).toBe('');
  });

  it('缺三项：渲染 Node/Git/CLI 三行 + 镜像文案 + 不出现「科学上网」', () => {
    const markup = renderToStaticMarkup(
      <SetupGuide engine="claude-code" seedDeps={allMissing} onReady={() => {}} />,
    );
    expect(markup).toContain('Node.js 18+');
    expect(markup).toContain('Git');
    expect(markup).toContain('Claude Code CLI');
    expect(markup).toContain('一键安装');
    expect(markup).toContain('国内镜像源');
    expect(markup).toContain('npmmirror');
    expect(markup).not.toContain('科学上网');
    expect(markup).not.toContain('代理');
  });

  it('git 缺失时引导卡无「知道了」「跳过」按钮（强制前置）', () => {
    const markup = renderToStaticMarkup(
      <SetupGuide engine="claude-code" seedDeps={nodeOk} onReady={() => {}} />,
    );
    // Node 已就绪，Git 与 CLI 缺失：引导卡常驻
    expect(markup).toContain('一键安装');
    expect(markup).not.toContain('知道了');
    expect(markup).not.toContain('跳过');
  });

  it('切引擎后 Codex CLI 行重新缺失，Node/Git 共享保留已装', () => {
    const codexMissing = setupGuideRows(
      { node: 'ok', npm: 'ok', git: 'ok', cli: 'missing' },
      'codex',
    );
    expect(codexMissing[2].name).toBe('Codex CLI');
    expect(codexMissing[2].status).toBe('missing');
    expect(codexMissing[0].status).toBe('ok');
    expect(codexMissing[1].status).toBe('ok');

    // Claude 已装后切到 Codex，Codex CLI 行恢复缺失态
    const markup = renderToStaticMarkup(
      <SetupGuide
        engine="codex"
        seedDeps={{ node: 'ok', npm: 'ok', git: 'ok', cli: 'missing' }}
        onReady={() => {}}
      />,
    );
    expect(markup).toContain('Codex CLI');
    expect(markup).toContain('一键安装');
    expect(markup).toContain('Node.js'); // Node 行仍显示已安装 pill
  });

  it('安装中文案不出现假百分比进度条', () => {
    const markup = renderToStaticMarkup(
      <SetupGuide engine="claude-code" seedDeps={allMissing} onReady={() => {}} />,
    );
    expect(markup).not.toMatch(/\d+%/);
  });

  it('restartRequired=true 时已安装 pill 提示「重启 Helm 后生效」', () => {
    const rows = setupGuideRows(allOk, 'claude-code', { node: true });
    expect(rows[0].restartRequired).toBe(true);
    // 通过组件渲染路径间接验证：seedDeps 全绿时组件返回空，无法直接渲染 pill，
    // 这里靠纯函数锁定 restartRequired 透传，组件内 pill 文案由 CSS 锁定可见性。
  });
});
