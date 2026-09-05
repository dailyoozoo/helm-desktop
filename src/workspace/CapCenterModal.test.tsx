import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { CapCenterModal } from './CapCenterModal';
import type { SlashCommand } from '../extensions/extensionsApi';

const commands: SlashCommand[] = [
  {
    id: '__helm_plan',
    trigger: '/plan',
    description: '编辑前先规划',
    scope: 'project',
    enabled: true,
    body: '',
    engine: 'claude-code',
    source: 'builtin',
  },
  {
    id: '__skill_review',
    trigger: '/code-review',
    description: '按团队规范审查未提交改动',
    scope: 'project',
    enabled: true,
    body: '',
    engine: 'claude-code',
    source: 'engine-user',
  },
];

describe('CapCenterModal', () => {
  it('关闭态（cap=null）不渲染任何弹层', () => {
    const markup = renderToStaticMarkup(
      <CapCenterModal
        cap={null}
        commands={commands}
        onClose={() => {}}
        onPickContext={() => {}}
        onPickCommand={() => {}}
        onPickNativeFile={() => {}}
      />,
    );
    expect(markup).toBe('');
  });

  it('文件与目录入口：标题/说明/搜索框/原生文件行（原型 CENTER.files）', () => {
    const markup = renderToStaticMarkup(
      <CapCenterModal
        cap="files"
        cwd="D:/work"
        commands={commands}
        onClose={() => {}}
        onPickContext={() => {}}
        onPickCommand={() => {}}
        onPickNativeFile={() => {}}
      />,
    );
    expect(markup).toContain('文件与目录');
    expect(markup).toContain('选择文件或目录。');
    expect(markup).toContain('搜索文件与目录');
    expect(markup).toContain('从电脑选择文件…');
    expect(markup).toContain('cm-modal-backdrop');
    expect(markup).toContain('cm-command-row');
  });

  it('命令与技能入口：按「内置命令 / 技能 Skills」分组，行带触发词与来源角标', () => {
    const markup = renderToStaticMarkup(
      <CapCenterModal
        cap="commands"
        commands={commands}
        onClose={() => {}}
        onPickContext={() => {}}
        onPickCommand={() => {}}
        onPickNativeFile={() => {}}
      />,
    );
    expect(markup).toContain('添加到任务');
    expect(markup).toContain('内置命令');
    expect(markup).toContain('技能 Skills');
    expect(markup).toContain('/plan');
    expect(markup).toContain('/code-review');
    expect(markup).toContain('技能');
    expect(markup).toContain('命令');
  });
});
